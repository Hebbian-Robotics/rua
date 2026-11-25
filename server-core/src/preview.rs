// INVARIANT: Preview uses a separate model and controller state: it commands no physical arm and writes no command records.

use std::time::{Duration, Instant};

use anyhow::{Context, ensure};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::arm::ReachGoal;
use crate::config::ControllerConfig;
use crate::motion_guards::{build_guard, build_self_collision, guard_command, load_shared_robot};
use crate::plant::{Feedback, NoPhysicsStandIn, Plant};
use crate::policy::{ActiveReach, PolicyRuntime, ReachStatus};
use crate::self_collision::SelfCollisionGuard;
use crate::surface_guard::SurfaceGuard;

// CONSTRAINT: Bound rehearsal wall time so a viewer drag cannot wait indefinitely.
const WALL_LIMIT: Duration = Duration::from_millis(1500);

// WHY: A rehearsal whose tool has stayed this still, for this long, short of the
// pose has stopped closing on it: rehearsing on to the deadline would end at
// the same joints.
const STALL_SECONDS: f64 = 0.5;
const STALL_TRANSLATION_METERS: f64 = 0.0005;
const STALL_ROTATION_RADIANS: f64 = 0.005;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PreviewOutcome {
    Settled,
    TimedOut,
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ReachPreview {
    pub outcome: PreviewOutcome,
    pub joints: Vec<f64>,
    pub position: [f64; 3],
    pub quaternion: [f64; 4],
    pub position_error_meters: f64,
    pub orientation_error_radians: f64,
    pub steps: u32,
    pub simulated_seconds: f64,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PreviewRequest {
    pub goal: ReachGoal,
    pub joints: Vec<f64>,
    pub gripper_angle: Option<f64>,
}

pub struct Preview {
    config: ControllerConfig,
    runtime: PolicyRuntime,
    guard: Option<SurfaceGuard>,
    self_collision: Option<SelfCollisionGuard>,
}

impl Preview {
    pub fn new(config: &ControllerConfig) -> anyhow::Result<Option<Self>> {
        if config.policy.is_none() {
            return Ok(None);
        }
        let robot = load_shared_robot(config)?;
        let Some(runtime) = PolicyRuntime::new(config, robot.as_ref())? else {
            return Ok(None);
        };
        Ok(Some(Self {
            config: config.clone(),
            runtime,
            guard: build_guard(config, robot.as_ref())?,
            self_collision: build_self_collision(config, robot.as_ref())?,
        }))
    }

    pub fn reach(&mut self, request: &PreviewRequest) -> anyhow::Result<ReachPreview> {
        let config = &self.config;
        ensure!(
            request.joints.len() == config.joint_count,
            "Expected {} joints, got {}",
            config.joint_count,
            request.joints.len()
        );
        let interval = 1.0 / config.frequency_hertz;
        let target = request.goal.target;
        let settings = config
            .policy
            .as_ref()
            .context("No policy execution settings")?;
        let mut plant = NoPhysicsStandIn::at(request.joints.clone(), settings.preview_lag_seconds);
        if let Some(angle) = request.gripper_angle {
            plant = plant.with_gripper(angle);
        }
        let mut previous_command = request.joints.clone();
        self.runtime.reset();
        let mut reach = ActiveReach::new(request.goal.clone(), settings);
        let started = Instant::now();
        let mut steps = 0_u32;
        let mut still_since: Option<(nalgebra::Isometry3<f64>, f64)> = None;
        let mut feedback = Feedback::default();
        loop {
            plant.read_feedback_into(&mut feedback)?;
            let tool = self.runtime.tool_pose(&feedback)?;
            let still_seconds = match &mut still_since {
                Some((anchor, seconds))
                    if (tool.translation.vector - anchor.translation.vector).norm()
                        <= STALL_TRANSLATION_METERS
                        && tool.rotation.angle_to(&anchor.rotation) <= STALL_ROTATION_RADIANS =>
                {
                    *seconds += interval;
                    *seconds
                }
                _ => {
                    still_since = Some((tool, 0.0));
                    0.0
                }
            };
            let mut ending = match reach.assess(&tool, &feedback.velocities, interval) {
                ReachStatus::Settled => Some((PreviewOutcome::Settled, None)),
                ReachStatus::TimedOut => Some((
                    PreviewOutcome::TimedOut,
                    Some("The policy did not settle on the pose before the deadline".to_owned()),
                )),
                ReachStatus::Running => None,
            };
            if ending.is_none() && still_seconds >= STALL_SECONDS {
                ending = Some((
                    PreviewOutcome::TimedOut,
                    Some("The policy stopped short of the pose".to_owned()),
                ));
            }
            if ending.is_none() && started.elapsed() > WALL_LIMIT {
                ending = Some((
                    PreviewOutcome::TimedOut,
                    Some("The preview's time limit passed before the policy settled".to_owned()),
                ));
            }
            if ending.is_none() {
                let joint_goal = self
                    .runtime
                    .joint_goal(config, &feedback, &target)
                    .context("The policy failed")?;
                match guard_command(
                    config,
                    self.guard.as_ref(),
                    self.self_collision.as_ref(),
                    &joint_goal,
                    &previous_command,
                    &feedback.positions,
                    feedback.gripper_angle,
                ) {
                    Ok(()) => {
                        plant.step(&joint_goal, interval);
                        previous_command.copy_from_slice(&joint_goal);
                    }
                    Err(trip) => ending = Some((PreviewOutcome::Blocked, Some(trip.to_string()))),
                }
            }
            if let Some((outcome, reason)) = ending {
                return self.result(
                    outcome,
                    reason,
                    &feedback,
                    &request.goal,
                    steps,
                    f64::from(steps) * interval,
                );
            }
            steps += 1;
        }
    }

    fn result(
        &self,
        outcome: PreviewOutcome,
        reason: Option<String>,
        feedback: &crate::plant::Feedback,
        goal: &ReachGoal,
        steps: u32,
        simulated_seconds: f64,
    ) -> anyhow::Result<ReachPreview> {
        let tool = self.runtime.tool_pose(feedback)?;
        let position = tool.translation.vector;
        let rotation = tool.rotation;
        let (position_error_meters, orientation_error_radians) = pose_errors(&tool, goal);
        Ok(ReachPreview {
            outcome,
            joints: feedback.positions.clone(),
            position: [position.x, position.y, position.z],
            quaternion: [rotation.i, rotation.j, rotation.k, rotation.w],
            position_error_meters,
            orientation_error_radians,
            steps,
            simulated_seconds,
            reason,
        })
    }
}

fn pose_errors(tool: &nalgebra::Isometry3<f64>, goal: &ReachGoal) -> (f64, f64) {
    (
        (tool.translation.vector - goal.target.translation.vector).norm(),
        tool.rotation.angle_to(&goal.target.rotation),
    )
}

//! The ghost's rehearsal of a reach: the same policy, arrival check
//! and guards that carry out a real `reach_pose` command, run against a
//! modelled arm as fast as the processor allows. It touches no arm: it starts from
//! joints the client supplies, commands nothing and writes no record, so a viewer
//! can ask while the real arm holds or moves.

use std::time::{Duration, Instant};

use anyhow::{Context, ensure};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::arm::ReachGoal;
use crate::config::ExecutorConfig;
use crate::control_loop::{build_guard, build_self_collision, guard_command, load_shared_robot};
use crate::kinematics::pose_from_xyzw;
use crate::limits::Trip;
use crate::plant::{Plant, SimulatedPlant};
use crate::policy::{ActiveReach, PolicyRuntime, ReachStatus};
use crate::self_collision::SelfCollisionGuard;
use crate::surface_guard::SurfaceGuard;

/// A rehearsal that has run this long stops, so a drag never waits on one.
const WALL_LIMIT: Duration = Duration::from_millis(1500);

/// How a rehearsed reach ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PreviewOutcome {
    /// The tool settled within both tolerances, as a real command would end.
    Settled,
    /// It did not settle before the command's deadline (or this rehearsal's
    /// time limit), or it stopped closing on the pose; the joints are where it
    /// had got to.
    TimedOut,
    /// A guard refused the next command; the joints are where the arm would
    /// have stopped.
    Blocked,
}

/// Where a rehearsed reach ended.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ReachPreview {
    pub outcome: PreviewOutcome,
    /// The arm joints where the rehearsal ended.
    pub joints: Vec<f64>,
    /// The tool pose at `joints`.
    pub position: [f64; 3],
    pub quaternion: [f64; 4],
    pub position_error_meters: f64,
    pub orientation_error_radians: f64,
    /// Control cycles rehearsed, and the time they stand for.
    pub steps: u32,
    pub simulated_seconds: f64,
    /// Why it did not settle.
    pub reason: Option<String>,
}

/// What to rehearse: the goal as a real command would carry it, and where the
/// arm starts.
#[derive(Debug, Clone, PartialEq)]
pub struct PreviewRequest {
    pub goal: ReachGoal,
    pub joints: Vec<f64>,
    pub gripper_angle: Option<f64>,
}

/// An arm's rehearsal machinery: its own copy of the policy and guards, so a
/// rehearsal never shares state with the control loop.
pub struct Preview {
    config: ExecutorConfig,
    runtime: PolicyRuntime,
    guard: Option<SurfaceGuard>,
    self_collision: Option<SelfCollisionGuard>,
}

impl Preview {
    /// The rehearsal machinery for this config; `None` when it has no policy.
    ///
    /// # Errors
    /// The policy or the robot model could not be loaded.
    pub fn new(config: &ExecutorConfig) -> anyhow::Result<Option<Self>> {
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

    /// Rehearse the reach from the joints given.
    ///
    /// # Errors
    /// The request is not usable (wrong joint count, a malformed pose), or the
    /// policy failed.
    pub fn reach(&mut self, request: &PreviewRequest) -> anyhow::Result<ReachPreview> {
        let config = &self.config;
        ensure!(
            request.joints.len() == config.joint_count,
            "Expected {} joints, got {}",
            config.joint_count,
            request.joints.len()
        );
        let interval = 1.0 / config.frequency_hertz;
        let target = pose_from_xyzw(request.goal.position, request.goal.quaternion)?;
        let settings = config
            .policy
            .as_ref()
            .context("No policy execution settings")?;
        let mut plant = SimulatedPlant::at(request.joints.clone(), settings.preview_lag_seconds);
        if let Some(angle) = request.gripper_angle {
            plant = plant.with_gripper(angle);
        }
        let mut previous_command = request.joints.clone();
        self.runtime.reset();
        let mut reach = ActiveReach::new(request.goal.clone(), settings);
        let started = Instant::now();
        let mut steps = 0_u32;
        loop {
            let feedback = plant.read_feedback()?;
            let tool = self.runtime.tool_pose(&feedback)?;
            let mut ending = match reach.assess_pose(&tool, &feedback, interval) {
                ReachStatus::Settled => Some((PreviewOutcome::Settled, None)),
                ReachStatus::TimedOut => Some((
                    PreviewOutcome::TimedOut,
                    Some("The policy did not settle on the pose before the deadline".to_owned()),
                )),
                ReachStatus::Running => None,
            };
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
                    &feedback,
                ) {
                    Ok(()) => {
                        plant.command(&joint_goal, interval)?;
                        previous_command = joint_goal;
                    }
                    Err(Trip(reason)) => ending = Some((PreviewOutcome::Blocked, Some(reason))),
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

/// The tool's distance from the goal position and the angle between its
/// orientation and the goal's.
fn pose_errors(tool: &nalgebra::Isometry3<f64>, goal: &ReachGoal) -> (f64, f64) {
    let rotation = tool.rotation;
    let alignment = [rotation.i, rotation.j, rotation.k, rotation.w]
        .iter()
        .zip(goal.quaternion)
        .map(|(a, b)| a * b)
        .sum::<f64>()
        .abs()
        .min(1.0);
    (
        (tool.translation.vector - nalgebra::Vector3::from(goal.position)).norm(),
        2.0 * alignment.acos(),
    )
}

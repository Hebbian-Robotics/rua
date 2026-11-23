use super::learned::{MotionPlan, PolicyRequest, PolicySession};
use super::tool_pose::ToolPose;
use super::trajectory::{ActiveGroupMember, ActiveMotion, GroupReference, MotionOutput};
use super::{build_guard, build_self_collision, clamp_setpoint, guard_command, load_shared_robot};
use crate::arm::{ArmCore, Now, StreamGoal};
use crate::config::ControllerConfig;
use crate::gripper::GripperController;
use crate::limits::{Trip, check_effort, validated_tracking_error};
use crate::plant::Feedback;
use crate::policy::{ActiveReach, PolicyRuntime};
use crate::reference_filter::{Reference, ReferenceFilter};
use crate::self_collision::SelfCollisionGuard;
use crate::surface_guard::SurfaceGuard;
use hebbian_contracts::CommandOutcome;

pub(super) struct ActiveStream {
    pub(super) command_id: String,
    pub(super) policy_engaged: bool,
}

pub(super) enum Activity {
    Idle,
    Motion(Box<ActiveMotion>),
    Group(Box<ActiveGroupMember>),
    Stream(ActiveStream),
    Reach(Box<ActiveReach>),
}

pub(super) struct Loop<'a> {
    pub(super) config: &'a ControllerConfig,
    pub(super) core: ArmCore,
    pub(super) filter: ReferenceFilter,
    pub(super) guard: Option<SurfaceGuard>,
    pub(super) self_collision: Option<std::sync::Arc<SelfCollisionGuard>>,
    pub(super) policy: Option<PolicyRuntime>,
    pub(super) tool: Option<ToolPose>,
    pub(super) activity: Activity,
    pub(super) previous_command: Vec<f64>,
    pub(super) commanded_velocities: Vec<f64>,
    pub(super) next_command: Vec<f64>,
    pub(super) reference: Reference,
    pub(super) hold_at: Option<Vec<f64>>,
    // INVARIANT: Why the arm last tripped, while the check that tripped it still fails.
    // A tripped arm already holds; tripping again would move that hold point
    // and repeat the event. It also refuses new motion until the check passes.
    pub(super) tripped: Option<Trip>,
    pub(super) shutdown_return_attempted: bool,
    pub(super) gripper: Option<GripperController>,
    pub(super) gripper_command: Option<String>,
}

impl Loop<'_> {
    pub(super) fn arm(&mut self) -> &mut ArmCore {
        &mut self.core
    }

    pub(super) fn powered(&self) -> bool {
        matches!(
            self.core.mode(),
            hebbian_contracts::ArmMode::Holding | hebbian_contracts::ArmMode::Moving
        )
    }

    pub(super) fn power_up(&mut self, positions: &[f64], gripper_angle: Option<f64>) {
        self.gripper = self
            .config
            .gripper
            .as_ref()
            .zip(gripper_angle)
            .map(|(limits, angle)| GripperController::new(limits, &self.config.motion, angle));
        self.gripper_command = None;
        self.filter.reset(positions);
        self.previous_command.clear();
        self.previous_command.extend_from_slice(positions);
        self.commanded_velocities.fill(0.0);
        self.hold_at = None;
        self.tripped = None;
        self.activity = Activity::Idle;
    }

    // INVARIANT: Torque is off: no goal, hold or trip survives it. The state machine ends
    // the command that was running.
    pub(super) fn power_down(&mut self) {
        self.activity = Activity::Idle;
        self.gripper = None;
        self.gripper_command = None;
        self.hold_at = None;
        self.tripped = None;
    }

    pub(super) fn cycle_failure(
        &self,
        feedback: &Feedback,
        interval: f64,
        deadline_missed: Option<String>,
    ) -> Option<Trip> {
        deadline_missed.map(Trip::Deadline).or_else(|| {
            validated_tracking_error(
                self.config,
                &feedback.positions,
                &self.previous_command,
                &self.commanded_velocities,
                interval,
            )
            .err()
            .map(|failure| {
                let motion_speed = match &self.activity {
                    Activity::Motion(active) if self.config.feetech.is_some() => {
                        Some((active.supervisor.peak_speed(), active.speed_fraction))
                    }
                    _ => None,
                };
                failure.into_trip(self.config, feedback, motion_speed)
            })
            .or_else(|| check_effort(self.config, &feedback.efforts).err())
            .or_else(|| feedback.fault.clone().map(Trip::FeedbackFault))
        })
    }

    pub(super) fn prepare_trip(&mut self, trip: Trip, feedback: &Feedback) -> String {
        self.filter.reset(&feedback.positions);
        self.hold_at = Some(feedback.positions.clone());
        self.activity = Activity::Idle;
        let reason = trip.detail().to_owned();
        self.tripped = Some(trip);
        reason
    }

    fn jaw_angle_refusal(
        &self,
        previous_angle: f64,
        angle: f64,
        feedback: &Feedback,
    ) -> anyhow::Result<Option<String>> {
        let positions = &feedback.positions;
        if let Some(guard) = &self.self_collision
            && let Some(reason) = guard.collision(positions, Some(angle))?
        {
            return Ok(Some(reason));
        }
        if let Some(guard) = &self.guard
            && let Some(reason) = guard.jaw_descent_reason(positions, previous_angle, angle)?
        {
            return Ok(Some(reason));
        }
        Ok(None)
    }

    // SAFETY: Refuse jaw motion if the arm is tripped or geometry checks cannot run; never substitute an unchecked target.
    pub(super) fn safe_jaw_target(
        &self,
        requested: f64,
        measured: f64,
        feedback: &Feedback,
    ) -> Result<f64, String> {
        if let Some(reason) = &self.tripped {
            return Err(format!("The arm is tripped: {reason}"));
        }
        let mut last_safe = measured;
        for fraction in crate::robot::path_sample_fractions(&[measured], &[requested]) {
            let candidate = measured + (requested - measured) * fraction;
            match self.jaw_angle_refusal(last_safe, candidate, feedback) {
                Err(error) => return Err(format!("{error:#}")),
                Ok(Some(_)) => break,
                Ok(None) => last_safe = candidate,
            }
        }
        Ok(last_safe)
    }

    pub(super) fn request_group_stop(&mut self, detail: &str) {
        if let Activity::Group(member) = &mut self.activity {
            member.stop_reason.get_or_insert_with(|| detail.to_owned());
        }
    }

    pub(super) fn start_stream(&mut self, command_id: String) {
        self.hold_at = None;
        self.activity = Activity::Stream(ActiveStream {
            command_id,
            policy_engaged: false,
        });
    }

    pub(super) fn end_activity(
        &mut self,
        outcome: CommandOutcome,
        detail: Option<String>,
        hold: Option<Vec<f64>>,
        now: Now,
    ) {
        self.activity = Activity::Idle;
        self.hold_at = hold;
        let hold = self
            .hold_at
            .get_or_insert_with(|| self.filter.stopping_position());
        self.core.motion_ended(outcome, detail, hold, now);
        self.reference.set_at_rest(hold, None);
    }

    fn stream_reference(&mut self, stop: bool, now: Now) -> Result<MotionPlan, Trip> {
        let Activity::Stream(stream) = &mut self.activity else {
            return Err(Trip::Trajectory("No stream is running".into()));
        };
        if stop {
            let effects = self.prepare_stop_write(Vec::new(), "Stopped".into());
            return Ok(MotionPlan::ReferenceReady(MotionOutput::Arm(effects)));
        }
        let command_id = stream.command_id.as_str();
        let timeout = self.config.control_timing.stream_goal_timeout;
        let timed_out = self.core.stream_timed_out(command_id, now.instant, timeout);
        match self.core.stream_goal(command_id) {
            None => Err(Trip::Trajectory("The stream has no goal".into())),
            Some(_) if timed_out => {
                let effects = self.prepare_stop_write(
                    Vec::new(),
                    format!("No goal for {} s", self.config.stream_goal_timeout_seconds),
                );
                Ok(MotionPlan::ReferenceReady(MotionOutput::Arm(effects)))
            }
            Some((
                StreamGoal::Joints {
                    positions,
                    joint_speed,
                },
                _,
            )) => {
                stream.policy_engaged = false;
                self.hold_at = None;
                self.reference
                    .set_at_rest(positions.as_slice(), Some(joint_speed.radians_per_second()));
                Ok(MotionPlan::ReferenceReady(MotionOutput::None))
            }
            Some((
                StreamGoal::Pose {
                    position,
                    quaternion,
                },
                _,
            )) => {
                self.hold_at = None;
                let session = if stream.policy_engaged {
                    PolicySession::Continue
                } else {
                    PolicySession::Reset
                };
                PolicyRequest::new(session, *position, *quaternion).map(MotionPlan::Infer)
            }
        }
    }

    pub(super) fn plan_motion(
        &mut self,
        interval: f64,
        feedback: &Feedback,
        group: Option<Result<&GroupReference, &Trip>>,
        now: Now,
    ) -> Result<MotionPlan, Trip> {
        let stop = self.core.stop_requested();
        let effects = match self.activity {
            Activity::Motion(_) => {
                MotionOutput::Arm(self.motion_reference(interval, feedback, stop)?)
            }
            Activity::Group(_) => self.group_reference(feedback, stop, group)?,
            Activity::Stream(_) => return self.stream_reference(stop, now),
            Activity::Reach(_) => return self.reach_reference(interval, feedback, stop),
            Activity::Idle => {
                let hold = self
                    .hold_at
                    .get_or_insert_with(|| self.filter.positions().to_vec());
                self.reference.set_at_rest(hold, None);
                MotionOutput::None
            }
        };
        Ok(MotionPlan::ReferenceReady(effects))
    }

    pub(super) fn guard_setpoint(
        &mut self,
        effects: &MotionOutput,
        interval: f64,
        feedback: &Feedback,
    ) -> Result<(), Trip> {
        let policy_command = matches!(self.activity, Activity::Reach(_))
            || (matches!(&self.activity, Activity::Stream(stream) if stream.policy_engaged)
                && !effects.ends_motion());
        // WHY: Smooth the terminal hold from the commanded velocity so a stop brakes
        // instead of freezing, including when completion still awaits the write.
        let smooth_destination =
            effects.ends_motion() || matches!(self.activity, Activity::Stream(_) | Activity::Idle);
        if smooth_destination && !policy_command {
            self.filter
                .apply(&self.reference, interval, &mut self.next_command)?;
        } else {
            self.filter.follow(&self.reference);
            self.next_command.clone_from(&self.reference.positions);
        }
        clamp_setpoint(self.config, &mut self.next_command, &self.previous_command);
        guard_command(
            self.config,
            self.guard.as_ref(),
            self.self_collision.as_deref(),
            &self.next_command,
            &self.previous_command,
            feedback,
        )
    }
}

pub(super) fn initialize_loop<'a>(
    config: &'a ControllerConfig,
    mut core: ArmCore,
    first: &Feedback,
) -> anyhow::Result<Loop<'a>> {
    let robot = load_shared_robot(config)?;
    let reference = Reference::at_rest(&first.positions, None);
    let self_collision = build_self_collision(config, robot.as_ref())?.map(std::sync::Arc::new);
    if let (Some(guard), Some(base)) = (&self_collision, &config.base_pose) {
        core.group_collision = Some(crate::self_collision::GroupCollisionArm {
            guard: guard.clone(),
            base: crate::kinematics::base_pose_isometry(base),
            gripper_angle: first.gripper_angle,
        });
    }
    Ok(Loop {
        config,
        core,
        filter: ReferenceFilter::new(config, &first.positions)?,
        guard: build_guard(config, robot.as_ref())?,
        self_collision,
        policy: PolicyRuntime::new(config, robot.as_ref())?,
        tool: ToolPose::new(config, robot.as_ref())?,
        activity: Activity::Idle,
        next_command: first.positions.clone(),
        commanded_velocities: vec![0.0; first.positions.len()],
        previous_command: first.positions.clone(),
        reference,
        hold_at: None,
        tripped: None,
        shutdown_return_attempted: false,
        gripper: None,
        gripper_command: None,
    })
}

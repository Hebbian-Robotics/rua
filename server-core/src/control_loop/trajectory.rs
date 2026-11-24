use super::activity::{Activity, Loop};
use crate::arm::{AdmittedPlan, GroupMembership, Now};
use crate::corner_path::{MIN_TRACKING_SPEED_FRACTION, MotionEnd, MotionSupervisor};
use crate::limits::{Trip, max_joint_deviation};
use crate::plant::Feedback;
use crate::reference_filter::Reference;
use hebbian_contracts::{Checkpoint, CommandOutcome, CommandPhase, GripperResult};

// WHY: Start easing off before the tracking guard trips. Ruckig applies the new
// cap from the current reference state, keeping its acceleration and jerk limits.
fn tracking_speed_fraction(
    feedback: &Feedback,
    commanded: &[f64],
    commanded_velocities: &[f64],
    following_lag: f64,
    static_margin: f64,
    requested_speed: f64,
) -> f64 {
    let mut fraction: f64 = 1.0;
    for (((measured, measured_velocity), commanded), commanded_velocity) in feedback
        .positions
        .iter()
        .zip(&feedback.velocities)
        .zip(commanded)
        .zip(commanded_velocities)
    {
        let lead = commanded - measured;
        if lead * commanded_velocity <= 0.0
            || lead * measured_velocity <= 0.0
            || lead.abs() < 0.02
            || measured_velocity.abs() < 0.1
        {
            continue;
        }
        let excess_lag = lead.abs() / measured_velocity.abs() - following_lag;
        if excess_lag > 0.0 {
            // WHY: Leave room for the guard allowance to shrink while braking.
            let speed_with_margin = 0.55 * static_margin / excess_lag;
            fraction = fraction
                .min((speed_with_margin / requested_speed).clamp(MIN_TRACKING_SPEED_FRACTION, 1.0));
        }
    }
    // WHY: Avoid replanning on tiny fluctuations in the velocity estimate.
    (fraction * 20.0).round() / 20.0
}

pub(super) struct ActiveMotion {
    pub(super) speed_fraction: f64,
    pub(super) supervisor: MotionSupervisor,
    pub(super) checkpoints_sent: usize,
    waypoint_gripper_command: Option<hebbian_contracts::CommandId>,
}

enum WaypointGripperStatus {
    Waiting,
    Complete,
    HomeFailed(String),
}

impl ActiveMotion {
    fn waypoint_gripper_status(
        &mut self,
        core: &crate::arm::ArmCore,
        gripper_limits: Option<&crate::config::GripperLimits>,
    ) -> Result<WaypointGripperStatus, Trip> {
        let Some(command_id) = &self.waypoint_gripper_command else {
            return Ok(WaypointGripperStatus::Complete);
        };
        let record = core
            .record(command_id)
            .ok_or_else(|| Trip::Gripper("Lost the waypoint jaw command record".into()))?;
        let Some(outcome) = record.outcome() else {
            return Ok(WaypointGripperStatus::Waiting);
        };
        if let Some(saved_angle) = self.supervisor.final_gripper_angle() {
            let reached_saved_angle = matches!(
                &record.phase,
                CommandPhase::Finished { completion, .. }
                    if matches!(completion.gripper, Some(GripperResult::Reached { measured_angle_radians })
                        if gripper_limits.is_some_and(|limits|
                            (measured_angle_radians - saved_angle.radians()).abs()
                                <= limits.position_tolerance_radians))
            );
            if outcome != CommandOutcome::Completed || !reached_saved_angle {
                let detail = record
                    .detail()
                    .unwrap_or("The jaw did not complete its move");
                return Ok(WaypointGripperStatus::HomeFailed(format!(
                    "Saved gripper opening was not reached (target {:.3} rad): {detail}",
                    saved_angle.radians()
                )));
            }
        } else if outcome != CommandOutcome::Completed {
            return Err(Trip::Trajectory(format!(
                "Waypoint gripper command ended with {outcome:?}"
            )));
        }
        self.waypoint_gripper_command = None;
        Ok(WaypointGripperStatus::Complete)
    }
}

// INVARIANT: This arm's part of a grouped motion. The group's one trajectory is stepped
// by the loop that runs every arm, not by the arm.
pub(super) struct ActiveGroupMember {
    pub(super) group: GroupMembership,
    pub(super) plan: crate::arm::AdmittedPlan,
    pub(super) stop_reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MotionStatus {
    Running,
    Reached,
    StoppedShort,
    TrajectoryTimedOut,
}

impl MotionStatus {
    pub(super) fn of(supervisor: &MotionSupervisor) -> Self {
        match supervisor.end() {
            None => Self::Running,
            Some(MotionEnd::Reached) => Self::Reached,
            Some(MotionEnd::StoppedShort) => Self::StoppedShort,
            Some(MotionEnd::TrajectoryTimedOut) => Self::TrajectoryTimedOut,
        }
    }
}

pub(super) struct MotionProgress {
    pub(super) checkpoints: Vec<Checkpoint>,
    pub(super) status: MotionStatus,
    pub(super) final_pose: Vec<f64>,
}

#[derive(Default)]
pub(super) struct MotionEffects {
    waypoint_gripper: Option<crate::arm::GripperTarget>,
    checkpoints: Vec<Checkpoint>,
    completion: Option<MotionCompletion>,
}

pub(super) enum MotionOutput {
    None,
    Arm(MotionEffects),
    Group {
        effects: MotionEffects,
        attempt: GroupWriteAttempt,
    },
}

impl MotionOutput {
    pub(super) fn ends_motion(&self) -> bool {
        match self {
            Self::None => false,
            Self::Arm(effects) | Self::Group { effects, .. } => effects.completion.is_some(),
        }
    }
}

struct MotionCompletion {
    outcome: CommandOutcome,
    detail: Option<String>,
    hold: Vec<f64>,
}

#[derive(Clone, Copy)]
pub(super) enum GroupWriteAttempt {
    First,
    Retry,
}

pub(super) struct GroupReference {
    pub(super) attempt: GroupWriteAttempt,
    pub(super) reference: Reference,
    pub(super) checkpoints: Vec<Checkpoint>,
    pub(super) segment: Option<&'static str>,
    pub(super) status: MotionStatus,
    pub(super) final_pose: Vec<f64>,
}

impl Loop<'_> {
    pub(super) fn start_motion(&mut self, plan: AdmittedPlan) -> anyhow::Result<()> {
        let supervisor = MotionSupervisor::from_validated(
            plan.into_validated(),
            &self.config.motion,
            self.config.tracking_following_lag_seconds,
        )?;
        self.hold_at = None;
        self.activity = Activity::Motion(Box::new(ActiveMotion {
            supervisor,
            speed_fraction: 1.0,
            checkpoints_sent: 0,
            waypoint_gripper_command: None,
        }));
        Ok(())
    }

    pub(super) fn start_group_member(
        &mut self,
        plan: crate::arm::AdmittedPlan,
        group: GroupMembership,
    ) {
        self.hold_at = None;
        self.activity = Activity::Group(Box::new(ActiveGroupMember {
            group,
            plan,
            stop_reason: None,
        }));
    }

    pub(super) fn motion_reference(
        &mut self,
        interval: f64,
        feedback: &Feedback,
        stop: bool,
    ) -> Result<MotionEffects, Trip> {
        let Activity::Motion(active) = &mut self.activity else {
            return Err(Trip::Trajectory("No motion is running".into()));
        };
        if stop {
            let checkpoints = active
                .supervisor
                .new_checkpoints(active.checkpoints_sent)
                .to_vec();
            return Ok(self.prepare_stop_write(checkpoints, "Stopped".into()));
        }
        if self.core.take_supersede() {
            // INVARIANT: A new command replaced this one while it only settled; the arm holds
            // where it measures, and the new motion plans from there.
            self.filter.reset(&feedback.positions);
            self.hold_at = Some(feedback.positions.clone());
            self.core.superseded_motion_ended(&feedback.positions);
            self.activity = Activity::Idle;
            self.reference.set_at_rest(&feedback.positions, None);
            return Ok(MotionEffects::default());
        }
        let speed_fraction = if self.config.feetech.is_some() {
            tracking_speed_fraction(
                feedback,
                &self.previous_command,
                &self.commanded_velocities,
                self.config.tracking_following_lag_seconds,
                self.config.tracking_static_margin_radians,
                active.supervisor.peak_speed(),
            )
        } else {
            1.0
        };
        active.supervisor.advance_with_speed_fraction(
            &feedback.positions,
            interval,
            speed_fraction,
        )?;
        active.speed_fraction = speed_fraction;
        self.reference.copy_from(active.supervisor.reference());
        let settling_at_end = active.supervisor.settling_at_end();
        let new = active
            .supervisor
            .new_checkpoints(active.checkpoints_sent)
            .to_vec();
        let mut status = MotionStatus::of(&active.supervisor);
        match active.waypoint_gripper_status(&self.core, self.config.gripper.as_ref())? {
            WaypointGripperStatus::Waiting if status == MotionStatus::Reached => {
                status = MotionStatus::Running;
            }
            WaypointGripperStatus::HomeFailed(detail) => {
                return Ok(self.prepare_measured_hold(
                    CommandOutcome::Failed,
                    Some(detail),
                    feedback,
                ));
            }
            WaypointGripperStatus::Waiting | WaypointGripperStatus::Complete => {}
        }
        self.core.set_segment(active.supervisor.segment_label());
        let gripper_angle = match status {
            _ if new.is_empty() => None,
            MotionStatus::Reached => active.supervisor.final_gripper_angle(),
            MotionStatus::Running => active.supervisor.waypoint_gripper(active.checkpoints_sent),
            MotionStatus::StoppedShort | MotionStatus::TrajectoryTimedOut => None,
        };
        let waypoint_gripper = gripper_angle
            .map(|angle| {
                let speed = hebbian_contracts::JointSpeed::new(active.supervisor.peak_speed())
                    .map_err(|error| Trip::Trajectory(error.to_string()))?;
                self.core
                    .check_gripper_target(angle, Some(speed))
                    .map_err(|failure| Trip::Gripper(failure.detail))
            })
            .transpose()?;
        if status == MotionStatus::Reached && waypoint_gripper.is_some() {
            status = MotionStatus::Running;
        }
        let progress = MotionProgress {
            checkpoints: new,
            status,
            final_pose: if status == MotionStatus::Running {
                Vec::new()
            } else {
                active.supervisor.final_pose().to_vec()
            },
        };
        self.core.set_settling(
            settling_at_end
                && active.waypoint_gripper_command.is_none()
                && waypoint_gripper.is_none(),
        );
        let mut effects = self.prepare_motion_effects(feedback, progress);
        effects.waypoint_gripper = waypoint_gripper;
        Ok(effects)
    }

    pub(super) fn prepare_stop_write(
        &mut self,
        checkpoints: Vec<Checkpoint>,
        detail: String,
    ) -> MotionEffects {
        self.prepare_braking_hold(checkpoints, CommandOutcome::Stopped, detail)
    }

    fn prepare_braking_hold(
        &mut self,
        checkpoints: Vec<Checkpoint>,
        outcome: CommandOutcome,
        detail: String,
    ) -> MotionEffects {
        let hold = self
            .hold_at
            .get_or_insert_with(|| self.filter.stopping_position());
        self.reference.set_at_rest(hold, None);
        MotionEffects {
            waypoint_gripper: None,
            checkpoints,
            completion: Some(MotionCompletion {
                outcome,
                detail: Some(detail),
                hold: hold.clone(),
            }),
        }
    }

    fn prepare_motion_effects(
        &mut self,
        feedback: &Feedback,
        progress: MotionProgress,
    ) -> MotionEffects {
        let MotionProgress {
            checkpoints,
            status,
            final_pose,
        } = progress;
        let (outcome, detail, hold) = match status {
            MotionStatus::Running => {
                return MotionEffects {
                    waypoint_gripper: None,
                    checkpoints,
                    completion: None,
                };
            }
            MotionStatus::TrajectoryTimedOut => {
                let error = max_joint_deviation(&feedback.positions, &final_pose);
                return self.prepare_braking_hold(
                    checkpoints,
                    CommandOutcome::TimedOut,
                    format!("Trajectory did not finish before its deadline; remaining joint error {error:.4} rad"),
                );
            }
            MotionStatus::StoppedShort => {
                let errors: Vec<String> = feedback
                    .positions
                    .iter()
                    .zip(&final_pose)
                    .map(|(measured, target)| format!("{:.3}", (measured - target).abs()))
                    .collect();
                (
                    CommandOutcome::StoppedShort,
                    format!(
                        "Settling deadline expired; remaining joint errors (rad): [{}]",
                        errors.join(", ")
                    ),
                    feedback.positions.clone(),
                )
            }
            MotionStatus::Reached => {
                let final_error =
                    max_joint_deviation(&feedback.positions, &self.reference.positions);
                (
                    CommandOutcome::Completed,
                    format!("Final joint error {final_error:.4} rad"),
                    final_pose,
                )
            }
        };
        self.reference.set_at_rest(&hold, None);
        MotionEffects {
            waypoint_gripper: None,
            checkpoints,
            completion: Some(MotionCompletion {
                outcome,
                detail: Some(detail),
                hold,
            }),
        }
    }

    pub(super) fn prepare_measured_hold(
        &mut self,
        outcome: CommandOutcome,
        detail: Option<String>,
        feedback: &Feedback,
    ) -> MotionEffects {
        let hold = feedback.positions.clone();
        self.filter.reset(&hold);
        self.reference.set_at_rest(&hold, None);
        MotionEffects {
            waypoint_gripper: None,
            checkpoints: Vec::new(),
            completion: Some(MotionCompletion {
                outcome,
                detail,
                hold,
            }),
        }
    }

    pub(super) fn commit_motion_effects(&mut self, effects: MotionEffects, now: Now) {
        if let Activity::Motion(active) = &mut self.activity {
            if let Some(target) = effects.waypoint_gripper {
                active.waypoint_gripper_command =
                    Some(self.core.apply_waypoint_gripper(target, now));
            }
            active.checkpoints_sent += effects.checkpoints.len();
        }
        for checkpoint in effects.checkpoints {
            self.core.add_checkpoint(checkpoint);
        }
        if let Some(completion) = effects.completion {
            self.end_activity(
                completion.outcome,
                completion.detail,
                Some(completion.hold),
                now,
            );
        }
    }

    pub(super) fn group_reference(
        &mut self,
        feedback: &Feedback,
        stop: bool,
        group: Option<Result<&GroupReference, &Trip>>,
    ) -> Result<MotionOutput, Trip> {
        let Activity::Group(member) = &self.activity else {
            return Err(Trip::Trajectory("No group motion is running".into()));
        };
        if let Some(reason) = member
            .stop_reason
            .clone()
            .or_else(|| stop.then(|| "Stopped".to_owned()))
        {
            return Ok(MotionOutput::Arm(
                self.prepare_stop_write(Vec::new(), reason),
            ));
        }
        let Some(share) = group else {
            self.reference.set_at_rest(self.filter.positions(), None);
            return Ok(MotionOutput::None);
        };
        let share = share.map_err(Clone::clone)?;
        self.reference.copy_from(&share.reference);
        self.core.set_segment(share.segment);
        let effects = self.prepare_motion_effects(
            feedback,
            MotionProgress {
                checkpoints: share.checkpoints.clone(),
                status: share.status,
                final_pose: share.final_pose.clone(),
            },
        );
        Ok(MotionOutput::Group {
            effects,
            attempt: share.attempt,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ControllerConfig, TEST_CONFIG};
    use crate::limits::validated_tracking_error;
    use crate::plant::{NoPhysicsStandIn, Plant};
    use hebbian_contracts::{CornerPlan, Segment};

    #[test]
    fn a_lagging_axis_can_finish_without_raising_the_tracking_limit() -> Result<(), Trip> {
        let mut input = ControllerConfig::parse(TEST_CONFIG).unwrap().into_input();
        input.tracking_following_lag_seconds = 0.11;
        input.tracking_static_margin_radians = 0.1;
        input.motion.acceleration_limit = 5.0;
        input.motion.jerk_limit = 20.0;
        let config = ControllerConfig::try_from(input).unwrap();
        let run = |adapt: bool, lag: f64| -> Result<(bool, f64, usize), Trip> {
            let mut plant = NoPhysicsStandIn::at(vec![0.0, 0.0], lag);
            let mut motion = MotionSupervisor::new(
                CornerPlan {
                    peak_speed: 2.0,
                    segments: vec![Segment {
                        final_gripper_angle: None,
                        pass_through: None,
                        label: "sweep".into(),
                        corners: vec![vec![0.0, 0.0], vec![1.8, 0.5]],
                        minimum_duration_seconds: None,
                        destination_position: None,
                        destination_quaternion: None,
                    }],
                },
                &config.motion,
                2,
            )
            .unwrap();
            let interval = 0.02;
            let mut previous = [0.0; 2];
            let mut velocity = [0.0; 2];
            let mut peak_speed: f64 = 0.0;
            for cycle in 0..300 {
                let feedback = plant.read_feedback().unwrap();
                if validated_tracking_error(
                    &config,
                    feedback.measured_positions(&config).unwrap(),
                    &previous,
                    &velocity,
                    interval,
                )
                .is_err()
                {
                    return Ok((false, peak_speed, cycle));
                }
                let fraction = if adapt {
                    tracking_speed_fraction(
                        &feedback,
                        &previous,
                        &velocity,
                        config.tracking_following_lag_seconds,
                        config.tracking_static_margin_radians,
                        2.0,
                    )
                } else {
                    1.0
                };
                motion.advance_with_speed_fraction(&feedback.positions, interval, fraction)?;
                let next = &motion.reference().positions;
                for joint in 0..2 {
                    velocity[joint] = (next[joint] - previous[joint]) / interval;
                    previous[joint] = next[joint];
                    peak_speed = peak_speed.max(velocity[joint].abs());
                }
                plant.step(next, interval);
                if motion.done() {
                    return Ok((motion.end() == Some(MotionEnd::Reached), peak_speed, cycle));
                }
            }
            Ok((false, peak_speed, 300))
        };

        let baseline = run(false, 0.18)?;
        let adjusted = run(true, 0.18)?;
        assert!(!baseline.0, "{baseline:?}");
        assert!(adjusted.0, "{adjusted:?}");
        assert_eq!(run(true, 0.02)?, run(false, 0.02)?);
        Ok(())
    }
}

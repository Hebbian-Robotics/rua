//! Which commands are admitted, and the records, queue and mode they change.

use super::state::{ArmCore, new_identifier};
use super::types::{
    GripperCommand, GripperTarget, GroupMembership, HeldStreamGoal, Job, Lifecycle, Now,
    PowerState, ReachGoal, Rejection, ShortOfTarget, ShutdownState, StreamGoal, rejection,
};
use crate::corner_path::validate_plan;
use crate::joint_limits::motion_speed;
use crate::planning::orientation::{from_xyzw, orientation_summary, pose_orientation_quaternion};
use hebbian_contracts::{
    Achieved, ArmMode, Command, CommandCompletion, CommandOutcome, CommandRecord, CornerPlan,
    Event, PoseOrientation, RejectionReason, TargetStatus,
};

/// Records kept for clients that ask about a command after it ended.
pub(super) const RETAINED_RECORDS: usize = 200;
/// A fault is cleared only on feedback this fresh, taken after the fault.
const FAULT_RECOVERY_FEEDBACK_MAX_AGE_SECONDS: f64 = 1.0;
const POSE_QUATERNION_TOLERANCE: f64 = 1e-5;

impl ArmCore {
    pub(super) fn busy(&self) -> bool {
        self.lifecycle != Lifecycle::Idle
    }

    pub(super) fn faulted_rejection(&self) -> Rejection {
        if self.mode() == ArmMode::Disconnected {
            return rejection(
                RejectionReason::Faulted,
                format!(
                    "{}; the bus is disconnected and reopened every second, then enable is accepted",
                    self.fault().unwrap_or("The bus stopped answering")
                ),
            );
        }
        rejection(
            RejectionReason::Faulted,
            format!(
                "{}; send clear_fault after inspecting the arm",
                self.fault().unwrap_or("The arm is faulted")
            ),
        )
    }

    pub(super) fn shutting_down() -> Rejection {
        rejection(RejectionReason::ArmBusy, "The controller is shutting down")
    }

    /// Why a motion cannot start now.
    pub(super) fn mode_rejection(&self) -> Option<Rejection> {
        if self.shutdown_requested() {
            return Some(Self::shutting_down());
        }
        if self.mode().refuses_everything() {
            return Some(self.faulted_rejection());
        }
        if self.busy() || self.mode() == ArmMode::Moving {
            return Some(rejection(
                RejectionReason::ArmBusy,
                "Another command is active; send stop first",
            ));
        }
        if self.mode() != ArmMode::Holding {
            return Some(rejection(
                RejectionReason::NotEnabled,
                "The arm is disabled; send enable, then repeat the command",
            ));
        }
        None
    }

    pub(super) fn new_record(
        &self,
        kind: &str,
        joint_speed: Option<f64>,
        now: Now,
    ) -> CommandRecord {
        let mut record = CommandRecord::new(new_identifier(), kind, now.unix_seconds, joint_speed);
        record.trace_context.clone_from(&self.trace_context);
        record
    }

    /// Keep and announce a record's latest state.
    pub(super) fn store(&mut self, record: CommandRecord) {
        let id = record.command_id.clone();
        if self.records.insert(id.clone(), record.clone()).is_none() {
            self.record_order.push_back(id.clone());
            while self.record_order.len() > RETAINED_RECORDS {
                if let Some(oldest) = self.record_order.pop_front() {
                    self.records.remove(&oldest);
                }
            }
        }
        if record.completion.is_some() {
            self.last_finished = Some(id);
        }
        (self.publish)(Event::Record(record));
    }

    pub(super) fn set_power(&mut self, power: PowerState) {
        self.transition_power(|_| power);
    }

    pub(super) fn transition_power(&mut self, transition: impl FnOnce(PowerState) -> PowerState) {
        let old_mode = self.mode();
        let previous = std::mem::replace(&mut self.power, PowerState::Disabled);
        self.power = transition(previous);
        let mode = self.mode();
        if old_mode != mode {
            (self.publish)(Event::Mode {
                mode,
                fault: self.fault().map(str::to_owned),
            });
        }
    }

    /// End a record: its outcome, and where the arm was against its target.
    pub(super) fn finish(
        &mut self,
        command_id: &str,
        outcome: CommandOutcome,
        detail: Option<String>,
        now: Now,
    ) {
        let Some(mut record) = self.records.get(command_id).cloned() else {
            return;
        };
        let is_active = self.active_command_id() == Some(command_id);
        record.completion = Some(CommandCompletion {
            finished_at: now.unix_seconds,
            outcome,
            detail,
        });
        record.achieved = if outcome == CommandOutcome::Interrupted {
            None
        } else {
            self.achieved(&record)
        };
        record.target_status = record.target_joints.as_ref().and_then(|_| {
            if outcome == CommandOutcome::Interrupted {
                return None;
            }
            Some(if outcome == CommandOutcome::Completed {
                TargetStatus::Reached
            } else if outcome == CommandOutcome::StoppedShort {
                TargetStatus::StoppedShort
            } else if outcome == CommandOutcome::Failed || !is_active {
                TargetStatus::NotStarted
            } else {
                TargetStatus::Partial
            })
        });
        self.store(record);
    }

    pub(super) fn achieved(&self, record: &CommandRecord) -> Option<Achieved> {
        let observed = self.observation.as_ref()?;
        let joints = observed.positions.clone();
        let (position, quaternion) = observed
            .tool_pose
            .map_or((None, None), |(position, quaternion)| {
                (Some(position), Some(quaternion))
            });
        Some(Achieved {
            sampled_at: observed.sampled_at,
            joint_error_radians: record.target_joints.as_ref().map(|target| {
                joints
                    .iter()
                    .zip(target)
                    .map(|(a, b)| (a - b).abs())
                    .fold(0.0, f64::max)
            }),
            joint_errors_radians: record.target_joints.as_ref().map(|target| {
                joints
                    .iter()
                    .zip(target)
                    .map(|(a, b)| (a - b).abs())
                    .collect()
            }),
            position_error_meters: position.zip(record.target_position).map(|(a, b)| {
                a.iter()
                    .zip(&b)
                    .map(|(x, y)| (x - y) * (x - y))
                    .sum::<f64>()
                    .sqrt()
            }),
            orientation_error_radians: quaternion.zip(record.target_quaternion).map(|(a, b)| {
                let dot: f64 = a.iter().zip(&b).map(|(x, y)| x * y).sum();
                2.0 * dot.abs().min(1.0).acos()
            }),
            orientation: quaternion.map(|quaternion| {
                let target = record.target_quaternion.map(from_xyzw);
                orientation_summary(
                    &from_xyzw(quaternion),
                    self.limits.jaw_closing_axis,
                    target.as_ref(),
                )
            }),
            joints,
            position,
            quaternion,
        })
    }

    /// Admit a command: its record, or why not.
    ///
    /// # Errors
    /// The arm's mode, queue or the command's own shape refuses it.
    pub(crate) fn submit(
        &mut self,
        command: Command,
        now: Now,
    ) -> Result<CommandRecord, Rejection> {
        if self.shutdown_requested() && !matches!(command, Command::Shutdown | Command::Stop) {
            return Err(Self::shutting_down());
        }
        match command {
            Command::Stop => Ok(self.submit_stop(now)),
            Command::ClearFault => self.submit_clear_fault(now),
            Command::Shutdown => Ok(self.submit_shutdown(now)),
            Command::Enable => self.submit_enable(now),
            Command::ApproveRecovery => self.submit_recovery_approval(now),
            Command::Disable => self.submit_disable(now),
            Command::Move {
                label,
                plan,
                joint_speed,
                path_limited,
                pose_limited,
            } => {
                let short = ShortOfTarget {
                    path_limited,
                    pose_limited,
                };
                self.submit_move(&label, plan, joint_speed, short, None, now)
            }
            Command::StreamJoints {
                positions,
                gripper_angle,
                joint_speed,
                stream_command_id,
            } => {
                // The loop owns the core exclusively: validate both targets before
                // mutation, so a refusal cannot leave half the request applied.
                if let Some(angle) = gripper_angle {
                    self.check_gripper(angle)?;
                }
                let record =
                    self.submit_stream_joints(positions, joint_speed, stream_command_id, now)?;
                if let Some(angle) = gripper_angle {
                    self.submit_gripper(angle, joint_speed, now)?;
                }
                Ok(record)
            }
            Command::StreamPose {
                position,
                quaternion,
                stream_command_id,
            } => self.submit_stream_pose(position, quaternion, stream_command_id, now),
            Command::ReachPose {
                label,
                position,
                orientation,
            } => self.submit_reach(&label, position, orientation, now),
            Command::SetGripper { angle, joint_speed } => {
                self.submit_gripper(angle, joint_speed, now)
            }
        }
    }

    pub(super) fn enqueue(&mut self, record: &CommandRecord, job: Job, replacing: bool) {
        self.lifecycle = if replacing {
            Lifecycle::Superseding(Some(job))
        } else {
            Lifecycle::Queued(job)
        };
        self.store(record.clone());
    }

    pub(super) fn submit_enable(&mut self, now: Now) -> Result<CommandRecord, Rejection> {
        if self.mode().refuses_everything() {
            return Err(self.faulted_rejection());
        }
        if !self.mode().accepts_enable() || self.busy() {
            return Err(rejection(
                RejectionReason::ArmBusy,
                "The arm is already enabled",
            ));
        }
        let record = self.new_record("enable", None, now);
        self.enqueue(
            &record,
            Job::Enable {
                command_id: record.command_id.clone(),
            },
            false,
        );
        Ok(record)
    }

    fn submit_recovery_approval(&mut self, now: Now) -> Result<CommandRecord, Rejection> {
        if self.mode() != ArmMode::Disconnected || self.busy() {
            return Err(rejection(
                RejectionReason::ArmBusy,
                "The arm is not waiting for recovery approval",
            ));
        }
        let record = self.new_record("approve_recovery", None, now);
        self.enqueue(
            &record,
            Job::ApproveRecovery {
                command_id: record.command_id.clone(),
            },
            false,
        );
        Ok(record)
    }

    pub(super) fn submit_disable(&mut self, now: Now) -> Result<CommandRecord, Rejection> {
        if let Some(refused) = self.mode_rejection() {
            return Err(refused);
        }
        let record = self.new_record("release", None, now);
        self.enqueue(
            &record,
            Job::Release {
                command_id: record.command_id.clone(),
            },
            false,
        );
        Ok(record)
    }

    pub(super) fn submit_stop(&mut self, now: Now) -> CommandRecord {
        self.request_stop();
        self.cancel_queued(now, "Stopped before start");
        // A queued stream was just dropped; a running one ends with the motion.
        if self.active_command_id().is_none() {
            self.stream = None;
        }
        // Stop includes the jaw: it must not keep moving toward its old target.
        self.gripper_stop_requested = true;
        if let Some(command) = self.gripper_command.take() {
            self.finish(
                &command.command_id,
                CommandOutcome::Stopped,
                Some("Stopped and holding".into()),
                now,
            );
        }
        let record = self.new_record("stop", None, now);
        let id = record.command_id.clone();
        self.store(record);
        self.finish(&id, CommandOutcome::Completed, None, now);
        self.records[&id].clone()
    }

    pub(super) fn request_stop(&mut self) {
        if let Lifecycle::Starting(id) | Lifecycle::Executing { command_id: id, .. } =
            &self.lifecycle
            && self
                .records
                .get(id)
                .is_some_and(|record| record.label != "enable" && record.label != "release")
        {
            self.lifecycle = Lifecycle::Stopping(id.clone());
        }
    }

    pub(super) fn cancel_queued(&mut self, now: Now, detail: &str) {
        let queued = match std::mem::replace(&mut self.lifecycle, Lifecycle::Idle) {
            Lifecycle::Queued(job) => Some(job),
            Lifecycle::Superseding(job) => {
                self.lifecycle = Lifecycle::Superseding(None);
                job
            }
            other => {
                self.lifecycle = other;
                None
            }
        };
        if let Some(job) = queued {
            let id = job.command_id().to_owned();
            self.finish(&id, CommandOutcome::Stopped, Some(detail.to_owned()), now);
            if self
                .stream
                .as_ref()
                .is_some_and(|stream| stream.command_id == id)
            {
                self.stream = None;
            }
        }
    }

    pub(super) fn submit_clear_fault(&mut self, now: Now) -> Result<CommandRecord, Rejection> {
        if self.mode() == ArmMode::Disconnected {
            return Err(self.faulted_rejection());
        }
        if self.mode() == ArmMode::Faulted {
            let fresh = self.observation.as_ref().is_some_and(|observed| {
                matches!(&self.power, PowerState::Faulted(fault) if observed.sampled_at > fault.entered_at)
                    && (0.0..=FAULT_RECOVERY_FEEDBACK_MAX_AGE_SECONDS)
                        .contains(&(now.unix_seconds - observed.sampled_at))
            });
            if !fresh {
                return Err(rejection(
                    RejectionReason::Faulted,
                    "No recent motor feedback since the fault. Check the arm's power and USB connection; restart the controller if feedback does not return.",
                ));
            }
            if let Some(observed) = &self.observation
                && let Some(fault) = &observed.fault
            {
                return Err(rejection(
                    RejectionReason::Faulted,
                    format!("Motors are not all healthy and disabled: {fault}"),
                ));
            }
            self.set_power(PowerState::Disabled);
        }
        let record = self.new_record("clear_fault", None, now);
        let id = record.command_id.clone();
        self.store(record);
        self.finish(&id, CommandOutcome::Completed, None, now);
        Ok(self.records[&id].clone())
    }

    pub(super) fn submit_shutdown(&mut self, now: Now) -> CommandRecord {
        if !self.shutdown_requested() {
            self.shutdown = ShutdownState::Stopping;
            self.request_stop();
            self.cancel_queued(now, "Stopped before start");
            if self.active_command_id().is_none() {
                self.stream = None;
            }
        }
        let record = self.new_record("shutdown", None, now);
        let id = record.command_id.clone();
        self.store(record);
        self.finish(&id, CommandOutcome::Completed, None, now);
        self.records[&id].clone()
    }

    /// Why this plan could not be admitted now, without admitting it.
    ///
    /// # Errors
    /// The arm's mode, or the plan's shape, limits or speed, refuse it.
    pub(crate) fn check_move(&self, plan: &CornerPlan, joint_speed: f64) -> Result<(), Rejection> {
        if let Some(refused) = self.mode_rejection() {
            return Err(refused);
        }
        self.check_move_shape(plan, joint_speed)
    }

    /// Whether a new move may replace the running motion: it has finished its
    /// path and only waits to settle, and nothing else is in the way.
    fn replaces_settling_motion(&self) -> bool {
        matches!(self.lifecycle, Lifecycle::Executing { settling: true, .. })
            && !self.shutdown_requested()
            && !self.mode().refuses_everything()
    }

    /// # Errors
    /// The plan's shape or speed, or not holding, refuse it.
    fn check_move_shape(&self, plan: &CornerPlan, joint_speed: f64) -> Result<(), Rejection> {
        if self.poses().is_none() {
            return Err(rejection(RejectionReason::NotEnabled, "Not holding"));
        }
        validate_plan(plan, self.limits.joint_count)
            .map_err(|error| rejection(RejectionReason::InvalidTarget, format!("{error:#}")))?;
        // Where the plan starts is not checked: admission splices it onto the
        // hold pose. The joint limits are the
        // planner's: it clamps the plan, so they are not checked here.
        if !(joint_speed.is_finite() && joint_speed > 0.0) {
            return Err(rejection(
                RejectionReason::InvalidTarget,
                "A joint speed must be positive and finite",
            ));
        }
        Ok(())
    }

    /// Admit a move; with a `group`, it is this arm's part of a grouped motion.
    ///
    /// # Errors
    /// As [`Self::check_move`].
    pub(crate) fn submit_move(
        &mut self,
        label: &str,
        mut plan: CornerPlan,
        joint_speed: f64,
        short: ShortOfTarget,
        group: Option<GroupMembership>,
        now: Now,
    ) -> Result<CommandRecord, Rejection> {
        let replacing = group.is_none() && self.replaces_settling_motion();
        if replacing {
            self.check_move_shape(&plan, joint_speed)?;
        } else {
            self.check_move(&plan, joint_speed)?;
        }
        // The plan was made from a snapshot of the hold pose; it starts from
        // where the arm actually holds, so a rounding or a newer hold cannot
        // make the trajectory jump. A motion that replaces a settling one starts
        // from the latest measured joints.
        let start = match (&self.observation, replacing) {
            (Some(observed), true) => Some(observed.positions.clone()),
            _ => self.poses().map(|poses| poses.hold.clone()),
        };
        if let (Some(start), Some(first)) = (start, plan.segments.first_mut()) {
            first.corners[0] = start;
        }
        let target = plan.segments.last().expect("a validated plan has segments");
        let mut record = self.new_record(label, Some(joint_speed), now);
        record.target_joints = target.corners.last().cloned();
        record.target_position = target.target_position;
        record.target_quaternion = target.target_quaternion;
        record.path_limited = short.path_limited;
        record.pose_limited = short.pose_limited;
        record.group_id = group.as_ref().map(|group| group.id.clone());
        let job = Job::Move {
            command_id: record.command_id.clone(),
            plan,
            group,
        };
        if replacing && let Some(old_id) = self.active_command_id().map(str::to_owned) {
            self.finish(
                &old_id,
                CommandOutcome::Superseded,
                Some(format!("Replaced by {} while settling", record.command_id)),
                now,
            );
        }
        self.enqueue(&record, job, replacing);
        Ok(record)
    }

    pub(super) fn submit_reach(
        &mut self,
        label: &str,
        position: [f64; 3],
        orientation: PoseOrientation,
        now: Now,
    ) -> Result<CommandRecord, Rejection> {
        let Some(settings) = self.limits.pose_goals else {
            return Err(rejection(
                RejectionReason::InvalidTarget,
                "This executor has no policy loaded, so it cannot reach a pose",
            ));
        };
        if let Some(refused) = self.mode_rejection() {
            return Err(refused);
        }
        if !position.iter().all(|value| value.is_finite()) {
            return Err(rejection(
                RejectionReason::InvalidTarget,
                "A pose goal needs a finite position",
            ));
        }
        let quaternion = pose_orientation_quaternion(orientation, self.limits.jaw_closing_axis)
            .map_err(|why| rejection(RejectionReason::InvalidTarget, why.to_string()))?;
        let goal = ReachGoal {
            position,
            quaternion,
            position_tolerance_meters: settings.position_tolerance_meters,
            orientation_tolerance_radians: settings.orientation_tolerance_radians,
        };
        let mut record = self.new_record(label, None, now);
        record.target_position = Some(goal.position);
        record.target_quaternion = Some(goal.quaternion);
        let job = Job::Reach {
            command_id: record.command_id.clone(),
            goal,
        };
        self.enqueue(&record, job, false);
        Ok(record)
    }

    pub(super) fn submit_stream_joints(
        &mut self,
        positions: Vec<f64>,
        joint_speed: Option<f64>,
        stream_command_id: Option<String>,
        now: Now,
    ) -> Result<CommandRecord, Rejection> {
        if positions.len() != self.limits.joint_count || positions.iter().any(|v| !v.is_finite()) {
            return Err(rejection(
                RejectionReason::InvalidTarget,
                format!(
                    "A goal needs {} finite joint positions",
                    self.limits.joint_count
                ),
            ));
        }
        let speed = motion_speed(
            joint_speed,
            self.limits.default_joint_speed,
            self.limits.max_joint_speed,
        )
        .map_err(|invalid| rejection(RejectionReason::InvalidTarget, invalid.to_string()))?;
        let goal: Vec<f64> = positions
            .iter()
            .zip(self.limits.lower.iter().zip(&self.limits.upper))
            .map(|(value, (lower, upper))| value.clamp(*lower, *upper))
            .collect();
        self.submit_stream(
            "stream_joints",
            StreamGoal::Joints {
                positions: goal.clone(),
                joint_speed: speed,
            },
            Some(goal),
            Some(speed),
            stream_command_id,
            now,
        )
    }

    pub(super) fn submit_stream_pose(
        &mut self,
        position: [f64; 3],
        quaternion: [f64; 4],
        stream_command_id: Option<String>,
        now: Now,
    ) -> Result<CommandRecord, Rejection> {
        if self.limits.pose_goals.is_none() {
            return Err(rejection(
                RejectionReason::InvalidTarget,
                "This executor has no policy loaded, so it cannot take a pose goal",
            ));
        }
        let norm = quaternion.iter().map(|v| v * v).sum::<f64>().sqrt();
        if position.iter().chain(&quaternion).any(|v| !v.is_finite())
            || (norm - 1.0).abs() > POSE_QUATERNION_TOLERANCE
        {
            return Err(rejection(
                RejectionReason::InvalidTarget,
                "A pose goal needs a finite position and a unit quaternion",
            ));
        }
        self.submit_stream(
            "stream_pose",
            StreamGoal::Pose {
                position,
                quaternion,
            },
            None,
            None,
            stream_command_id,
            now,
        )
    }

    /// Start a stream, or replace the goal of a running one.
    pub(super) fn submit_stream(
        &mut self,
        kind: &str,
        goal: StreamGoal,
        goal_joints: Option<Vec<f64>>,
        joint_speed: Option<f64>,
        stream_command_id: Option<String>,
        now: Now,
    ) -> Result<CommandRecord, Rejection> {
        if let Some(id) = stream_command_id {
            return self.update_stream(&id, kind, goal, goal_joints, now);
        }
        if let Some(refused) = self.mode_rejection() {
            return Err(refused);
        }
        let mut record = self.new_record(kind, joint_speed, now);
        record.goal_joints = goal_joints;
        self.stream = Some(HeldStreamGoal {
            command_id: record.command_id.clone(),
            goal,
            last_goal_at: now.instant,
        });
        let job = Job::Stream {
            command_id: record.command_id.clone(),
        };
        self.enqueue(&record, job, false);
        Ok(record)
    }

    pub(super) fn update_stream(
        &mut self,
        id: &str,
        kind: &str,
        goal: StreamGoal,
        goal_joints: Option<Vec<f64>>,
        now: Now,
    ) -> Result<CommandRecord, Rejection> {
        let running = self
            .stream
            .as_mut()
            .filter(|stream| stream.command_id == id);
        let Some(stream) = running else {
            let ended = self.records.get(id);
            let outcome = ended.and_then(CommandRecord::outcome).map_or_else(
                || "unknown".to_owned(),
                |outcome| format!("{outcome:?}").to_lowercase(),
            );
            let detail = ended
                .and_then(CommandRecord::detail)
                .map(|detail| format!(": {detail}"))
                .unwrap_or_default();
            return Err(rejection(
                RejectionReason::StreamEnded,
                format!(
                    "Stream {id} ended ({outcome}{detail}); send a goal without a stream id to start another"
                ),
            ));
        };
        if std::mem::discriminant(&stream.goal) != std::mem::discriminant(&goal) {
            return Err(rejection(
                RejectionReason::InvalidTarget,
                "A stream cannot change between joint and pose goals",
            ));
        }
        stream.goal = goal;
        stream.last_goal_at = now.instant;
        let mut record = self.records[id].clone();
        record.goal_joints = goal_joints;
        if record.label != kind {
            return Err(rejection(
                RejectionReason::InvalidTarget,
                "A stream cannot change its kind",
            ));
        }
        self.store(record.clone());
        Ok(record)
    }

    /// The jaw target the gripper is sent: `angle` clamped into its range.
    fn check_gripper(&self, angle: f64) -> Result<f64, Rejection> {
        if self.shutdown_requested() {
            return Err(Self::shutting_down());
        }
        if self.mode().refuses_everything() {
            return Err(self.faulted_rejection());
        }
        let Some((minimum, maximum)) = self.limits.gripper else {
            return Err(rejection(
                RejectionReason::InvalidTarget,
                "This arm has no gripper",
            ));
        };
        if self.poses().is_none() {
            return Err(rejection(
                RejectionReason::NotEnabled,
                "The arm is disabled; send enable, then repeat the command",
            ));
        }
        if !angle.is_finite() {
            return Err(rejection(
                RejectionReason::InvalidTarget,
                "A gripper goal must be finite",
            ));
        }
        Ok(angle.clamp(minimum, maximum))
    }

    pub(super) fn submit_gripper(
        &mut self,
        angle: f64,
        requested_speed: Option<f64>,
        now: Now,
    ) -> Result<CommandRecord, Rejection> {
        let angle = self.check_gripper(angle)?;
        let speed = motion_speed(
            requested_speed,
            self.limits.default_joint_speed,
            self.limits.max_joint_speed,
        )
        .map_err(|invalid| rejection(RejectionReason::InvalidTarget, invalid.to_string()))?;
        if let Some(replaced) = self.gripper_command.take() {
            self.finish(
                &replaced.command_id,
                CommandOutcome::Stopped,
                Some("Replaced by a newer gripper target".into()),
                now,
            );
        }
        let record = self.new_record("set_gripper", Some(speed), now);
        self.gripper_command = Some(GripperCommand {
            command_id: record.command_id.clone(),
            pending_target: Some(GripperTarget {
                angle_radians: angle,
                max_speed_radians_per_second: speed,
            }),
        });
        self.store(record.clone());
        Ok(record)
    }
}

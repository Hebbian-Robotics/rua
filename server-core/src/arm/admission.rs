use super::motion_admission::{MotionScope, MoveDecision, MoveDisposition};
use super::state::{ArmCore, new_identifier};
use super::types::{
    CommandState, GripperCommand, GripperTarget, GroupMembership, HeldStreamGoal, Job, Now,
    PowerState, ReachGoal, Rejection, ShortOfTarget, ShutdownState, StreamGoal, rejection,
};
use crate::motion_values::JointSpeed;
use crate::planning::orientation::{from_xyzw, orientation_summary, pose_orientation_quaternion};
use hebbian_contracts::JawAngle;
use hebbian_contracts::{
    Achieved, ArmMode, Command, CommandCompletion, CommandOutcome, CommandPhase, CommandPose,
    CommandRecord, ControlAction, CornerPlan, GripperResult, KnownCommandOutcome, LoggedEvent,
    NotStartedOutcome, PoseOrientation, RecordedCommand, RejectionReason,
};

pub(super) const RETAINED_RECORDS: usize = 200;
// SAFETY: A fault is cleared only on feedback this fresh, taken after the fault.
const FAULT_RECOVERY_FEEDBACK_MAX_AGE_SECONDS: f64 = 1.0;
const POSE_QUATERNION_TOLERANCE: f64 = 1e-5;

// WHY: Joint errors alone do not tell a model why a move stalled or where to aim next;
// where the tool rests does. A limited move's segment claims no target position, so the
// distance left is given only when there is one. Each joint's error stays in `achieved`.
fn stopped_short_summary(achieved: &Achieved) -> Option<String> {
    let [x, y, z] = achieved.position?.map(|meters| meters * 1000.0);
    let shortfall = achieved
        .position_error_meters
        .map_or(String::new(), |meters| {
            format!(" {:.1} mm", meters * 1000.0)
        });
    Some(format!(
        "Stopped{shortfall} short of the target and no longer closing on it, with the tool at \
         [{x:.1}, {y:.1}, {z:.1}] mm; something is likely in the way, such as the table or \
         an object"
    ))
}

impl ArmCore {
    pub(super) fn busy(&self) -> bool {
        self.command_state != CommandState::Idle
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
        label: &str,
        command: RecordedCommand,
        now: Now,
    ) -> CommandRecord {
        let mut record = CommandRecord::new(new_identifier(), label, now.unix_seconds, command);
        record.trace_context.clone_from(&self.trace_context);
        record
    }

    pub(super) fn store(&mut self, record: CommandRecord) {
        let id = record.command_id.clone();
        if self.records.insert(id.clone(), record.clone()).is_none() {
            self.record_order.push_back(id.clone());
            while self.record_order.len() > RETAINED_RECORDS {
                let Some(completed) = self.record_order.iter().position(|id| {
                    self.records
                        .get(id)
                        .is_some_and(|record| !record.is_pending())
                }) else {
                    break;
                };
                if let Some(oldest) = self.record_order.remove(completed) {
                    self.records.remove(&oldest);
                }
            }
        }
        if !record.is_pending() {
            self.last_finished = Some(id);
        }
        (self.publish)(LoggedEvent::record(record));
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
            (self.publish)(LoggedEvent::mode(mode, self.fault().map(str::to_owned)));
        }
    }

    pub(super) fn finish(
        &mut self,
        command_id: &str,
        outcome: CommandOutcome,
        detail: Option<String>,
        now: Now,
    ) {
        self.finish_with_gripper_result(command_id, outcome, detail, None, now);
    }

    pub(super) fn finish_with_gripper_result(
        &mut self,
        command_id: &str,
        outcome: CommandOutcome,
        detail: Option<String>,
        gripper: Option<GripperResult>,
        now: Now,
    ) {
        let Some(mut record) = self.records.get(command_id).cloned() else {
            return;
        };
        record.phase = match outcome.known() {
            None => CommandPhase::OutcomeUnknown {
                finished_at: now.unix_seconds,
                detail,
            },
            Some(KnownCommandOutcome::Failed)
                if !self.motion_advanced
                    && matches!(
                        record.command,
                        RecordedCommand::Move { .. }
                            | RecordedCommand::PolicyReach { .. }
                            | RecordedCommand::JointStream { .. }
                            | RecordedCommand::PoseStream { .. }
                    ) =>
            {
                CommandPhase::NotStarted {
                    finished_at: now.unix_seconds,
                    outcome: NotStartedOutcome::Failed,
                    detail,
                }
            }
            Some(outcome) => {
                let achieved = self.achieved(&record);
                let detail = if outcome == KnownCommandOutcome::StoppedShort {
                    achieved.as_ref().and_then(stopped_short_summary).or(detail)
                } else {
                    detail
                };
                CommandPhase::Finished {
                    completion: CommandCompletion {
                        finished_at: now.unix_seconds,
                        outcome,
                        detail,
                        gripper,
                    },
                    achieved: achieved.map(Box::new),
                }
            }
        };
        self.store(record);
    }

    fn stop_before_start(&mut self, command_id: &str, detail: &str, now: Now) {
        if let Some(mut record) = self.records.get(command_id).cloned() {
            record.phase = CommandPhase::NotStarted {
                finished_at: now.unix_seconds,
                outcome: NotStartedOutcome::Stopped,
                detail: Some(detail.to_owned()),
            };
            self.store(record);
        }
    }

    pub(super) fn stop_gripper_command(
        &mut self,
        command: &GripperCommand,
        detail: &str,
        now: Now,
    ) {
        if command.pending_target.is_some() {
            self.stop_before_start(&command.command_id, detail, now);
        } else {
            self.finish(
                &command.command_id,
                CommandOutcome::Stopped,
                Some(detail.to_owned()),
                now,
            );
        }
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
            sampled_at: observed.sampled_at.unix_seconds,
            joint_error_radians: record.target_joints().map(|target| {
                joints
                    .iter()
                    .zip(target)
                    .map(|(a, b)| (a - b).abs())
                    .fold(0.0, f64::max)
            }),
            joint_errors_radians: record.target_joints().map(|target| {
                joints
                    .iter()
                    .zip(target)
                    .map(|(a, b)| (a - b).abs())
                    .collect()
            }),
            position_error_meters: position.zip(record.target_position()).map(|(a, b)| {
                a.iter()
                    .zip(&b)
                    .map(|(x, y)| (x - y) * (x - y))
                    .sum::<f64>()
                    .sqrt()
            }),
            orientation_error_radians: quaternion.zip(record.target_quaternion()).map(|(a, b)| {
                let dot: f64 = a.iter().zip(&b).map(|(x, y)| x * y).sum();
                2.0 * dot.abs().min(1.0).acos()
            }),
            orientation: quaternion.map(|quaternion| {
                let target = record.target_quaternion().map(from_xyzw);
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
                // INVARIANT: The loop owns the core exclusively: validate both targets before
                // mutation, so a refusal cannot leave half the request applied.
                let gripper = gripper_angle
                    .map(|angle| self.check_gripper_target(angle, joint_speed))
                    .transpose()?;
                let record =
                    self.submit_stream_joints(positions, joint_speed, stream_command_id, now)?;
                if let Some(target) = gripper {
                    self.apply_gripper(target, Some(record.command_id.clone()), now);
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
        self.command_state = if replacing {
            CommandState::Superseding(Some(job))
        } else {
            CommandState::Queued(job)
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
                if self.busy() {
                    "Another command is active; wait for it to finish"
                } else {
                    "The arm is already enabled"
                },
            ));
        }
        let record = self.new_record(
            "enable",
            RecordedCommand::Control {
                action: ControlAction::Enable,
            },
            now,
        );
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
        let record = self.new_record(
            "approve_recovery",
            RecordedCommand::Control {
                action: ControlAction::ApproveRecovery,
            },
            now,
        );
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
        let record = self.new_record(
            "release",
            RecordedCommand::Control {
                action: ControlAction::Disable,
            },
            now,
        );
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
        // INVARIANT: A queued stream was just dropped; a running one ends with the motion.
        if self.active_command_id().is_none() {
            self.stream = None;
        }
        // INVARIANT: Stop includes the jaw: it must not keep moving toward its old target.
        self.gripper_stop_requested = true;
        if let Some(command) = self.gripper_command.take() {
            self.stop_gripper_command(&command, "Stopped and holding", now);
        }
        let record = self.new_record(
            "stop",
            RecordedCommand::Control {
                action: ControlAction::Stop,
            },
            now,
        );
        let id = record.command_id.clone();
        self.store(record);
        self.finish(&id, CommandOutcome::Completed, None, now);
        self.records[&id].clone()
    }

    pub(crate) fn cancel_command(&mut self, command_id: &str, now: Now) {
        if self.active_command_id() == Some(command_id) {
            self.request_stop();
        }
        if matches!(
            &self.command_state,
            CommandState::Queued(job) | CommandState::Superseding(Some(job))
                if job.command_id() == command_id
        ) {
            self.cancel_queued(now, "Cancelled before start");
        }
        if self.gripper_command.as_ref().is_some_and(|command| {
            command.command_id == command_id || command.stream_id.as_deref() == Some(command_id)
        }) && let Some(command) = self.gripper_command.take()
        {
            self.gripper_stop_requested = true;
            self.stop_gripper_command(&command, "Cancelled and holding", now);
        }
    }

    pub(super) fn request_stop(&mut self) {
        if let CommandState::Starting(id) | CommandState::Executing { command_id: id, .. } =
            &self.command_state
            && self.records.get(id).is_some_and(|record| {
                !matches!(
                    record.command,
                    RecordedCommand::Control {
                        action: ControlAction::Enable | ControlAction::Disable
                    }
                )
            })
        {
            self.command_state = CommandState::Stopping(id.clone());
        }
    }

    pub(super) fn cancel_queued(&mut self, now: Now, detail: &str) {
        let queued = match std::mem::replace(&mut self.command_state, CommandState::Idle) {
            CommandState::Queued(job) => Some(job),
            CommandState::Superseding(job) => {
                self.command_state = CommandState::Superseding(None);
                job
            }
            other @ (CommandState::Idle
            | CommandState::Starting(_)
            | CommandState::Executing { .. }
            | CommandState::Stopping(_)) => {
                self.command_state = other;
                None
            }
        };
        if let Some(job) = queued {
            let id = job.command_id().to_owned();
            self.stop_before_start(&id, detail, now);
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
                matches!(&self.power, PowerState::Faulted(fault) if observed.sampled_at.instant > fault.entered_at)
                    && now.instant.checked_duration_since(observed.sampled_at.instant)
                        .is_some_and(|age| age.as_secs_f64() <= FAULT_RECOVERY_FEEDBACK_MAX_AGE_SECONDS)
            });
            if !fresh {
                return Err(rejection(
                    RejectionReason::Faulted,
                    "No recent motor feedback since the fault. Check the arm's power and USB connection. Restart the motor controller if feedback does not return.",
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
        let record = self.new_record(
            "clear_fault",
            RecordedCommand::Control {
                action: ControlAction::ClearFault,
            },
            now,
        );
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
        let record = self.new_record(
            "shutdown",
            RecordedCommand::Control {
                action: ControlAction::Shutdown,
            },
            now,
        );
        let id = record.command_id.clone();
        self.store(record);
        self.finish(&id, CommandOutcome::Completed, None, now);
        self.records[&id].clone()
    }

    pub(super) fn replaces_settling_motion(&self) -> bool {
        matches!(
            self.command_state,
            CommandState::Executing { settling: true, .. }
        ) && !self.shutdown_requested()
            && !self.mode().refuses_everything()
    }

    pub(crate) fn submit_move(
        &mut self,
        label: &str,
        plan: CornerPlan,
        joint_speed: JointSpeed,
        short: ShortOfTarget,
        group: Option<GroupMembership>,
        now: Now,
    ) -> Result<CommandRecord, Rejection> {
        let decision = MoveDecision::prepare(
            self,
            plan,
            joint_speed,
            if group.is_some() {
                MotionScope::Grouped
            } else {
                MotionScope::Individual
            },
        )?;
        Ok(self.apply_move(label, decision, short, group, now))
    }

    pub(crate) fn apply_move(
        &mut self,
        label: &str,
        decision: MoveDecision,
        short: ShortOfTarget,
        group: Option<GroupMembership>,
        now: Now,
    ) -> CommandRecord {
        let MoveDecision {
            plan,
            speed,
            disposition,
        } = decision;
        let replacing = matches!(disposition, MoveDisposition::ReplaceSettling { .. });
        let target = plan.final_segment();
        let record = self.new_record(
            label,
            RecordedCommand::Move {
                target_joints: plan.final_corner().to_vec(),
                target_position: target.target_position,
                target_quaternion: target.target_quaternion,
                joint_speed: speed.radians_per_second(),
                path_limited: short.path_limited,
                pose_limited: short.pose_limited,
                group_id: group.as_ref().map(|group| group.id.as_str().to_owned()),
            },
            now,
        );
        let job = Job::Move {
            command_id: record.command_id.clone(),
            plan,
            group,
        };
        if let MoveDisposition::ReplaceSettling { command_id: old_id } = disposition {
            self.finish(
                &old_id,
                CommandOutcome::Superseded,
                Some(format!("Replaced by {} while settling", record.command_id)),
                now,
            );
        }
        self.enqueue(&record, job, replacing);
        record
    }

    pub(crate) fn check_reach(
        &self,
        position: [f64; 3],
        orientation: PoseOrientation,
    ) -> Result<ReachGoal, Rejection> {
        let Some(settings) = self.limits.pose_goals else {
            return Err(rejection(
                RejectionReason::InvalidTarget,
                "This controller has no policy loaded, so it cannot reach a pose",
            ));
        };
        if let Some(refused) = self.mode_rejection() {
            return Err(refused);
        }
        let quaternion = pose_orientation_quaternion(orientation, self.limits.jaw_closing_axis)
            .map_err(|why| rejection(RejectionReason::InvalidTarget, why.to_string()))?;
        Ok(ReachGoal {
            target: crate::kinematics::pose_from_xyzw(position, quaternion)
                .map_err(|error| rejection(RejectionReason::InvalidTarget, error.to_string()))?,
            position_tolerance_meters: settings.position_tolerance_meters,
            orientation_tolerance_radians: settings.orientation_tolerance_radians,
        })
    }

    pub(super) fn submit_reach(
        &mut self,
        label: &str,
        position: [f64; 3],
        orientation: PoseOrientation,
        now: Now,
    ) -> Result<CommandRecord, Rejection> {
        let goal = self.check_reach(position, orientation)?;
        Ok(self.apply_reach(label, goal, now))
    }

    pub(crate) fn apply_reach(&mut self, label: &str, goal: ReachGoal, now: Now) -> CommandRecord {
        let record = self.new_record(
            label,
            RecordedCommand::PolicyReach {
                target_pose: CommandPose {
                    position: goal.target.translation.vector.into(),
                    quaternion: goal.target.rotation.coords.into(),
                },
            },
            now,
        );
        let job = Job::Reach {
            command_id: record.command_id.clone(),
            goal,
        };
        self.enqueue(&record, job, false);
        record
    }

    pub(super) fn submit_stream_joints(
        &mut self,
        positions: Vec<f64>,
        joint_speed: Option<JointSpeed>,
        stream_command_id: Option<String>,
        now: Now,
    ) -> Result<CommandRecord, Rejection> {
        let positions = crate::joint_limits::JointTarget::parse(
            positions,
            self.limits.joint_bounds.lower().len(),
            self.limits.joint_bounds.lower(),
            self.limits.joint_bounds.upper(),
        )
        .map_err(|error| rejection(RejectionReason::InvalidTarget, error.to_string()))?;
        let requested_speed = JointSpeed::for_request(
            joint_speed,
            self.limits.default_joint_speed,
            self.limits.max_joint_speed,
        )
        .map_err(|invalid| rejection(RejectionReason::InvalidTarget, invalid.to_string()))?;
        let speed = stream_command_id
            .as_ref()
            .and_then(|id| {
                self.stream
                    .as_ref()
                    .filter(|stream| &stream.command_id == id)
            })
            .and_then(|stream| stream.goal.joint_speed())
            .unwrap_or(requested_speed);
        self.submit_stream(
            StreamGoal::Joints {
                positions,
                joint_speed: speed,
            },
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
                "This controller has no policy loaded, so it cannot take a pose goal",
            ));
        }
        let target = hebbian_robot_geometry::kinematics::pose_with_tolerance(
            position,
            quaternion,
            POSE_QUATERNION_TOLERANCE,
        )
        .map_err(|error| rejection(RejectionReason::InvalidTarget, error.to_string()))?;
        self.submit_stream(StreamGoal::Pose(target), stream_command_id, now)
    }

    pub(super) fn submit_stream(
        &mut self,
        goal: StreamGoal,
        stream_command_id: Option<String>,
        now: Now,
    ) -> Result<CommandRecord, Rejection> {
        if let Some(id) = stream_command_id {
            return self.update_stream(&id, goal, now);
        }
        if let Some(refused) = self.mode_rejection() {
            return Err(refused);
        }
        let record = self.new_record(goal.label(), goal.recorded(), now);
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
        goal: StreamGoal,
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
        let mut record = self.records[id].clone();
        record.command = goal.recorded();
        stream.goal = goal;
        stream.last_goal_at = now.instant;
        self.store(record.clone());
        Ok(record)
    }

    fn check_gripper(&self, angle: JawAngle) -> Result<JawAngle, Rejection> {
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
        JawAngle::try_from(angle.radians().clamp(minimum, maximum))
            .map_err(|error| rejection(RejectionReason::InvalidTarget, error.to_string()))
    }

    pub(crate) fn check_gripper_target(
        &self,
        angle: JawAngle,
        requested_speed: Option<JointSpeed>,
    ) -> Result<GripperTarget, Rejection> {
        let angle = self.check_gripper(angle)?;
        let speed = JointSpeed::for_request(
            requested_speed,
            self.limits.default_joint_speed,
            self.limits.max_joint_speed,
        )
        .map_err(|invalid| rejection(RejectionReason::InvalidTarget, invalid.to_string()))?;
        Ok(GripperTarget { angle, speed })
    }

    pub(super) fn submit_gripper(
        &mut self,
        angle: JawAngle,
        requested_speed: Option<JointSpeed>,
        now: Now,
    ) -> Result<CommandRecord, Rejection> {
        let target = self.check_gripper_target(angle, requested_speed)?;
        Ok(self.apply_gripper(target, None, now))
    }

    pub(crate) fn apply_gripper(
        &mut self,
        target: GripperTarget,
        stream_command_id: Option<String>,
        now: Now,
    ) -> CommandRecord {
        if let Some(replaced) = self.gripper_command.take() {
            self.stop_gripper_command(&replaced, "Replaced by a newer gripper target", now);
        }
        let record = self.new_record(
            "set_gripper",
            RecordedCommand::Gripper {
                target_angle_radians: target.angle.radians(),
                joint_speed: target.speed.radians_per_second(),
                stream_command_id: stream_command_id.clone(),
            },
            now,
        );
        self.gripper_command = Some(GripperCommand {
            command_id: record.command_id.clone(),
            pending_target: Some(target),
            stream_id: stream_command_id,
        });
        self.store(record.clone());
        record
    }
}

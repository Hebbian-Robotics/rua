//! One arm's cycle: read, check, command, trace and publish.

use std::time::{Duration, Instant};

use super::activity::Activity;
use super::activity::Loop;
use super::trajectory::GroupReference;
use super::{LONGEST_RECORDED_CYCLE_MICROSECONDS, Report};
use crate::arm::{Job, Now};
use crate::config::ExecutorConfig;
use crate::control_link::{Link, SampleRecycler};
use crate::gripper::GripperState;
use crate::handoff::ArmInbox;
use crate::joint_limits::clamp_within_recovery;
use crate::limits::Trip;
use crate::limits::max_joint_deviation;
use crate::plant::{Feedback, Plant, Reconnection};
use crate::trace::TraceWriter;
use hebbian_contracts::ArmRecoveryIssue;
use hebbian_contracts::{ArmMode, Command, CommandOutcome, Event};
/// How long a bus that keeps failing is waited out, holding the last goal,
/// before the active command is interrupted and recovery begins.
const BUS_FAILURE_GRACE_SECONDS: f64 = 0.5;
/// A cycle longer than this many periods is fed to the generators as this
/// many, so a late cycle does not become a jump.
const LONGEST_STEP_PERIODS: f64 = 2.0;

pub(super) enum BusRecovery {
    Disabled,
    Reconnecting,
    VerifyingHold { feedback: Feedback, since: Instant },
    AwaitingApproval(ArmRecoveryIssue),
}

/// An arm's cycle after its reading and before its command.
pub(super) struct PendingCycle {
    pub(super) cycle_began: f64,
    pub(super) interval: f64,
    pub(super) now: Now,
    pub(super) failure: Option<Trip>,
    pub(super) read_time: f64,
    pub(super) restart_timer: bool,
}

/// How the first half of a cycle ended.
pub(super) enum Begun {
    /// Nothing is left to do this cycle.
    Done(Flow, bool),
    Pending(PendingCycle),
}

/// What a cycle asks of the loop around it.
pub(super) enum Flow {
    Continue,
    /// Shutdown finished: the arm is released and nothing is left to do.
    End,
}

/// One run of the loop: the plant, the clients' link and everything a cycle
/// reads or writes. A cycle is one call of `cycle`.
pub(super) struct Runner<'a, P: Plant> {
    /// What the station calls this arm, which a grouped move names.
    pub(super) name: String,
    pub(super) plant: &'a mut P,
    pub(super) link: Option<&'a Link>,
    /// What the link's readers have asked of this arm, answered at the top of each cycle.
    pub(super) inbox: Option<ArmInbox>,
    /// Samples the link's writer has finished with, refilled for the next cycle.
    pub(super) samples: Option<SampleRecycler>,
    /// This cycle's reading, kept so each cycle fills the same buffers.
    pub(super) feedback: Feedback,
    pub(super) config: &'a ExecutorConfig,
    pub(super) state: Loop<'a>,
    pub(super) trace: TraceWriter,
    pub(super) timings: crate::trace::CycleTimings,
    pub(super) report: Report,
    pub(super) started: Instant,
    pub(super) cycle_number: u64,
    /// The plant that paced the loop failed, so the loop has nothing to run on.
    pub(super) plant_lost: bool,
    /// Consecutive cycles in which the bus failed while the arm held a goal.
    pub(super) failed_cycles: u32,
    /// Why the last reconnection attempt failed, so a repeat is not logged again.
    pub(super) last_reconnect_failure: Option<String>,
    pub(super) recovery: BusRecovery,
    /// With no clients, the stream a timing run drives with its sine.
    pub(super) sine_stream: Option<String>,
}

impl<P: Plant> Runner<'_, P> {
    /// The plant failed: nothing can be checked or commanded. An enabled arm is
    /// released as well as the bus allows and enters the faulted mode; a
    /// disabled one only keeps the reason in the report.
    pub(super) fn plant_failed(&mut self, failure: &anyhow::Error, now: Now) {
        self.plant_lost = self.plant.pacing().is_set_by_replies();
        let reason = format!("{failure:#}");
        self.report.trip.get_or_insert_with(|| reason.clone());
        if self.state.powered {
            let cleanup_errors: Vec<String> = self
                .plant
                .release()
                .err()
                .map(|error| format!("{error:#}"))
                .into_iter()
                .collect();
            self.state.power_down();
            self.state.arm().enter_fault(&reason, &cleanup_errors, now);
        }
    }

    /// Interrupt the active motion without issuing a torque-off command.
    fn bus_lost(&mut self, failure: &anyhow::Error, now: Now) {
        if self.plant.pacing().is_set_by_replies() {
            self.plant_failed(failure, now);
            return;
        }
        let reason = format!("{failure:#}");
        self.report.trip.get_or_insert_with(|| reason.clone());
        self.recovery = if self.state.powered {
            BusRecovery::Reconnecting
        } else {
            BusRecovery::Disabled
        };
        self.state.power_down();
        eprintln!("{}: bus disconnected, retrying: {reason}", self.name);
        self.state.arm().bus_lost(&reason, &[], now);
    }

    fn reconnect_step(&mut self, feedback: &mut Feedback, now: Now) {
        if !matches!(self.recovery, BusRecovery::Disabled)
            && let Err(failure) = self.plant.heartbeat()
        {
            let reason = format!("Guard unavailable during recovery: {failure:#}");
            self.recovery = BusRecovery::AwaitingApproval(ArmRecoveryIssue {
                motors_without_torque: Vec::new(),
                detail: reason.clone(),
            });
            self.state.arm().enter_fault(&reason, &[], now);
            return;
        }
        self.take_recovery_request(now);
        if let BusRecovery::VerifyingHold {
            feedback: held,
            since,
        } = &self.recovery
        {
            let (held, since) = (held.clone(), *since);
            self.verify_recovery_hold(feedback, now, &held, since);
            return;
        }
        if matches!(self.recovery, BusRecovery::AwaitingApproval(_)) {
            return;
        }
        match self.plant.reconnect() {
            Reconnection::Waiting => {}
            Reconnection::Failed(failure) => {
                let reason = format!("{failure:#}");
                if self.last_reconnect_failure.as_deref() != Some(reason.as_str()) {
                    eprintln!("{}: bus still disconnected: {reason}", self.name);
                    self.last_reconnect_failure = Some(reason);
                }
            }
            Reconnection::Restored => self.inspect_reconnected_arm(),
        }
    }

    fn inspect_reconnected_arm(&mut self) {
        if matches!(self.recovery, BusRecovery::Disabled) {
            self.finish_disabled_reconnection();
            return;
        }
        let observation = match self.plant.observe_recovery() {
            Ok(observation) => observation,
            Err(failure) => {
                if failure.is::<crate::plant::CommunicationFailure>() {
                    self.recovery = BusRecovery::Reconnecting;
                    return;
                }
                self.recovery = BusRecovery::AwaitingApproval(ArmRecoveryIssue {
                    motors_without_torque: Vec::new(),
                    detail: format!("Could not inspect the arm: {failure:#}"),
                });
                return;
            }
        };
        let motors_without_torque = observation
            .motor_torque
            .iter()
            .filter_map(|(id, enabled)| (!enabled).then_some(*id))
            .collect::<Vec<_>>();
        if observation.motor_torque.is_empty()
            || !motors_without_torque.is_empty()
            || observation.health_issue.is_some()
        {
            self.recovery = BusRecovery::AwaitingApproval(ArmRecoveryIssue {
                motors_without_torque,
                detail: observation.health_issue.unwrap_or_else(|| {
                    if observation.motor_torque.is_empty() {
                        "Motor torque state is unavailable".into()
                    } else {
                        "One or more motors lost torque".into()
                    }
                }),
            });
            return;
        }
        if clamp_within_recovery(
            &observation.feedback.positions,
            &self.config.lower_limits,
            &self.config.upper_limits,
            self.config.joint_limit_recovery_tolerance_radians,
        )
        .is_err()
        {
            self.recovery = BusRecovery::AwaitingApproval(ArmRecoveryIssue {
                motors_without_torque: Vec::new(),
                detail: "A joint is outside its recovery limits".into(),
            });
            return;
        }
        match self.plant.hold_recovery() {
            Ok(observation)
                if clamp_within_recovery(
                    &observation.feedback.positions,
                    &self.config.lower_limits,
                    &self.config.upper_limits,
                    self.config.joint_limit_recovery_tolerance_radians,
                )
                .is_ok() =>
            {
                self.recovery = BusRecovery::VerifyingHold {
                    feedback: observation.feedback,
                    since: Instant::now(),
                };
            }
            Ok(_) => {
                self.recovery = BusRecovery::AwaitingApproval(ArmRecoveryIssue {
                    motors_without_torque: Vec::new(),
                    detail: "A joint is outside its recovery limits".into(),
                });
            }
            Err(failure) => {
                if failure.is::<crate::plant::CommunicationFailure>() {
                    self.recovery = BusRecovery::Reconnecting;
                    return;
                }
                self.recovery = BusRecovery::AwaitingApproval(ArmRecoveryIssue {
                    motors_without_torque: Vec::new(),
                    detail: format!("Could not establish a recovery hold: {failure:#}"),
                });
            }
        }
    }

    fn finish_disabled_reconnection(&mut self) {
        self.failed_cycles = 0;
        self.last_reconnect_failure = None;
        self.state.arm().bus_reconnected();
    }

    fn take_recovery_request(&mut self, now: Now) {
        if let Some(job) = self.state.arm().next_job() {
            if matches!(job, Job::ApproveRecovery { .. })
                && matches!(self.recovery, BusRecovery::AwaitingApproval(_))
            {
                self.approve_recovery(now);
            } else {
                self.state
                    .arm()
                    .failed("The arm is not waiting for recovery approval", now);
            }
        }
    }

    fn approve_recovery(&mut self, now: Now) {
        let attempt = (|| -> anyhow::Result<Feedback> {
            let observation = self.plant.observe_recovery()?;
            anyhow::ensure!(
                observation.health_issue.is_none(),
                "Recovery health check failed: {:?}",
                observation.health_issue
            );
            clamp_within_recovery(
                &observation.feedback.positions,
                &self.config.lower_limits,
                &self.config.upper_limits,
                self.config.joint_limit_recovery_tolerance_radians,
            )
            .map_err(|error| {
                anyhow::anyhow!("Joint {} is outside its recovery limits", error.joint)
            })?;
            let drift_limit = self
                .config
                .enable_checks
                .as_ref()
                .map_or(self.config.motion.settled_error_radians, |checks| {
                    checks.preflight_drift_limit_radians
                });
            Ok(self.plant.enable_recovery(drift_limit)?.feedback)
        })();
        match attempt {
            Ok(feedback) => {
                self.recovery = BusRecovery::VerifyingHold {
                    feedback,
                    since: Instant::now(),
                }
            }
            Err(error) => {
                let detail = format!("Approved recovery failed: {error:#}");
                self.state.arm().failed(&detail, now);
                if error.is::<crate::plant::CommunicationFailure>() {
                    self.recovery = BusRecovery::Reconnecting;
                    return;
                }
                self.recovery = BusRecovery::AwaitingApproval(ArmRecoveryIssue {
                    motors_without_torque: Vec::new(),
                    detail,
                });
            }
        }
    }

    fn verify_recovery_hold(
        &mut self,
        feedback: &mut Feedback,
        now: Now,
        held: &Feedback,
        since: Instant,
    ) {
        if self.state.core.stop_requested() {
            let detail = "Recovery approval was stopped before the hold was verified";
            self.state.arm().failed(detail, now);
            self.recovery = BusRecovery::AwaitingApproval(ArmRecoveryIssue {
                motors_without_torque: Vec::new(),
                detail: detail.into(),
            });
            return;
        }
        match self.plant.observe_recovery() {
            Ok(observation)
                if !observation.motor_torque.is_empty()
                    && observation.motor_torque.iter().all(|(_, enabled)| *enabled)
                    && observation.health_issue.is_none()
                    && max_joint_deviation(&held.positions, &observation.feedback.positions)
                        <= self.config.motion.settled_error_radians
                    && match (held.gripper_angle, observation.feedback.gripper_angle) {
                        (Some(before), Some(after)) => {
                            (before - after).abs() <= self.config.motion.settled_error_radians
                        }
                        (None, None) => true,
                        _ => false,
                    } =>
            {
                if since.elapsed().as_secs_f64() >= self.config.motion.settle_dwell_seconds {
                    *feedback = observation.feedback;
                    self.state
                        .power_up(&feedback.positions, feedback.gripper_angle);
                    self.state.arm().enabled(&feedback.positions, now);
                    let tool_pose = self
                        .state
                        .tool
                        .as_ref()
                        .and_then(|tool| tool.pose(feedback));
                    self.state.core.observe(
                        now.unix_seconds,
                        &feedback.positions,
                        tool_pose,
                        feedback.fault.as_deref(),
                    );
                    self.failed_cycles = 0;
                    self.last_reconnect_failure = None;
                    self.recovery = BusRecovery::Disabled;
                    eprintln!("{}: recovered at a measured hold", self.name);
                }
            }
            Ok(observation) => {
                self.state
                    .arm()
                    .failed("The arm did not remain at the recovery hold", now);
                self.recovery = BusRecovery::AwaitingApproval(ArmRecoveryIssue {
                    motors_without_torque: observation
                        .motor_torque
                        .iter()
                        .filter_map(|(id, enabled)| (!enabled).then_some(*id))
                        .collect(),
                    detail: observation
                        .health_issue
                        .unwrap_or_else(|| "The arm did not remain at the recovery hold".into()),
                });
            }
            Err(failure) => {
                self.state.arm().failed(
                    &format!("Could not verify the recovery hold: {failure:#}"),
                    now,
                );
                if failure.is::<crate::plant::CommunicationFailure>() {
                    self.recovery = BusRecovery::Reconnecting;
                    return;
                }
                self.recovery = BusRecovery::AwaitingApproval(ArmRecoveryIssue {
                    motors_without_torque: Vec::new(),
                    detail: format!("Could not verify the recovery hold: {failure:#}"),
                });
            }
        }
    }

    /// A bus failure: an enabled arm's servos keep holding their last goal,
    /// so nothing new is commanded and the bus is tried again next cycle; only
    /// a failure that lasts the grace period counts as the bus lost.
    pub(super) fn bus_failed(&mut self, failure: &anyhow::Error, now: Now) {
        self.failed_cycles += 1;
        let patient = !self.plant.pacing().is_set_by_replies()
            && f64::from(self.failed_cycles)
                < BUS_FAILURE_GRACE_SECONDS * self.config.frequency_hertz;
        if !patient {
            self.bus_lost(failure, now);
        } else if let Err(heartbeat_failure) = self.plant.heartbeat() {
            // The heartbeat is not on the bus, and the guard must keep hearing it:
            // a guard that is gone leaves the arm unwatched, so patience ends.
            self.plant_failed(&heartbeat_failure, now);
        }
    }

    /// Act on the jaw goal the state machine holds, if any.
    pub(super) fn drive_gripper(
        &mut self,
        feedback: &Feedback,
        interval: f64,
        now: Now,
    ) -> anyhow::Result<()> {
        let Some(measured) = feedback.gripper_angle else {
            return Ok(());
        };
        if self.state.gripper.is_none() {
            return Ok(());
        }
        if self.state.arm().take_gripper_stop() {
            // The record already ended stopped; the jaw holds where it is.
            self.state.gripper_command = None;
            if let Some(jaw) = &mut self.state.gripper {
                jaw.hold(measured);
            }
        }
        let new_target = self.state.arm().take_gripper_target();
        if let Some((command_id, gripper_target)) = new_target {
            if let Some(jaw) = &mut self.state.gripper {
                jaw.set_target(
                    gripper_target.angle_radians,
                    gripper_target.max_speed_radians_per_second,
                    measured,
                );
            }
            self.state.gripper_command = Some(command_id);
        }
        let Some(jaw) = &mut self.state.gripper else {
            return Ok(());
        };
        let proposed_command = jaw.step(measured, interval)?;
        let target = jaw.target();
        // Check only this cycle's smoothed travel, rather than scanning the
        // entire opening in one motor cycle. Recheck at the current arm pose
        // while arm and jaw move together.
        let command = match self
            .state
            .safe_jaw_target(proposed_command, measured, feedback)
        {
            Ok(command) => command,
            Err(reason) => {
                if let Some(jaw) = &mut self.state.gripper {
                    jaw.hold(measured);
                }
                if let Some(command_id) = self.state.gripper_command.take() {
                    self.state.arm().finish_gripper(
                        &command_id,
                        CommandOutcome::Failed,
                        Some(reason),
                        now,
                    );
                }
                return Ok(());
            }
        };
        let jaw = self
            .state
            .gripper
            .as_mut()
            .expect("the jaw was stepped above");
        let clipped = (command - proposed_command).abs() > 1e-8;
        if clipped {
            jaw.hold(command);
        }
        let state = jaw.state();
        let write_started = Instant::now();
        self.plant.command_gripper(command)?;
        self.timings.gripper_write_seconds = write_started.elapsed().as_secs_f64();
        if state != GripperState::Moving
            && let Some(command_id) = self.state.gripper_command.take()
        {
            let detail = if state == GripperState::Reached && !clipped {
                format!("Reached {measured:.3} rad")
            } else {
                format!(
                    "Stopped at {measured:.3} rad short of {target:.3}: holding an object or blocked"
                )
            };
            self.state.arm().finish_gripper(
                &command_id,
                CommandOutcome::Completed,
                Some(detail),
                now,
            );
        }
        Ok(())
    }

    /// With no clients, keep a stream going toward a slow sine on every joint,
    /// for timing runs.
    pub(super) fn drive_sine(&mut self, now: Now) {
        let positions =
            vec![0.4 * self.started.elapsed().as_secs_f64().sin(); self.config.joint_count];
        let arm = self.state.arm();
        if !matches!(arm.mode(), ArmMode::Holding | ArmMode::Moving) {
            return;
        }
        let command = Command::StreamJoints {
            gripper_angle: None,
            positions,
            joint_speed: None,
            stream_command_id: self.sine_stream.clone(),
        };
        if let Ok(record) = arm.submit(command, now) {
            self.sine_stream = Some(record.command_id);
        }
    }

    /// The first half of a cycle: read the plant, take the next command and
    /// drive the jaw. A grouped motion's trajectory is stepped between the two
    /// halves, once every arm has read. The reading stays in `self.feedback`.
    pub(super) fn begin_cycle(
        &mut self,
        cycle: Duration,
        deadline_missed: Option<String>,
    ) -> Begun {
        // Taken out of the runner so the cycle can borrow it while it borrows
        // the runner; put back before the cycle returns.
        let mut feedback = std::mem::take(&mut self.feedback);
        let begun = self.begin_cycle_with(&mut feedback, cycle, deadline_missed);
        self.feedback = feedback;
        begun
    }

    fn begin_cycle_with(
        &mut self,
        feedback: &mut Feedback,
        cycle: Duration,
        deadline_missed: Option<String>,
    ) -> Begun {
        let cycle_began = self.started.elapsed().as_secs_f64();
        let longest_step = LONGEST_STEP_PERIODS / self.config.frequency_hertz;
        if cycle.as_secs_f64() > longest_step {
            self.report.overruns += 1;
        }
        let interval = cycle.as_secs_f64().min(longest_step);
        let now = Now::current();
        self.report.cycle_microseconds.saturating_record(
            u64::try_from(cycle.as_micros()).unwrap_or(LONGEST_RECORDED_CYCLE_MICROSECONDS),
        );
        if self.state.arm().mode() == ArmMode::Disconnected {
            self.reconnect_step(feedback, now);
            // The last reading stands; the sample's mode says it is stale.
            self.publish(feedback, interval, false);
            return Begun::Done(self.finished_or_continue(), false);
        }
        if let Err(failure) = self.plant.read_feedback_into(feedback) {
            self.bus_failed(&failure, now);
            if matches!(
                self.state.arm().mode(),
                ArmMode::Disconnected | ArmMode::Faulted
            ) {
                self.publish(feedback, interval, false);
            }
            return Begun::Done(self.finished_or_continue(), false);
        }
        let read_time = self.started.elapsed().as_secs_f64();
        let tool_pose = self
            .state
            .tool
            .as_ref()
            .and_then(|tool| tool.pose(feedback));
        self.state.core.observe(
            now.unix_seconds,
            &feedback.positions,
            tool_pose,
            feedback.fault.as_deref(),
        );
        if self.link.is_none() {
            self.drive_sine(now);
        }

        let failure = self
            .state
            .cycle_failure(feedback, interval, deadline_missed);
        if failure.is_none() {
            self.state.tripped = None;
        }
        let start_next_began = Instant::now();
        let restart_timer = if matches!(self.state.activity, Activity::Idle) {
            self.start_next(now)
        } else {
            false
        };
        self.timings.start_next_seconds = start_next_began.elapsed().as_secs_f64();
        let gripper_began = Instant::now();
        if let Err(failure) = self.drive_gripper(feedback, interval, now) {
            self.bus_failed(&failure, now);
            return Begun::Done(self.finished_or_continue(), false);
        }
        self.timings.gripper_seconds = gripper_began.elapsed().as_secs_f64();
        if !self.state.powered {
            self.failed_cycles = 0;
            // A paced plant (a simulator) moves time on only when told to.
            if let Err(failure) = self.plant.advance(interval) {
                self.plant_failed(&failure, now);
                return Begun::Done(self.finished_or_continue(), false);
            }
            self.publish(feedback, interval, false);
            return Begun::Done(self.finished_or_continue(), restart_timer);
        }
        Begun::Pending(PendingCycle {
            cycle_began,
            interval,
            now,
            failure,
            read_time,
            restart_timer,
        })
    }

    /// The second half of a cycle: check, command the plant, trace and publish.
    /// `group` is this arm's share of its group's reference, when it is in one.
    pub(super) fn finish_cycle(
        &mut self,
        pending: PendingCycle,
        group: Option<&Result<GroupReference, Trip>>,
    ) -> (Flow, bool) {
        let feedback = std::mem::take(&mut self.feedback);
        let finished = self.finish_cycle_with(&feedback, pending, group);
        self.feedback = feedback;
        finished
    }

    fn finish_cycle_with(
        &mut self,
        feedback: &Feedback,
        pending: PendingCycle,
        group: Option<&Result<GroupReference, Trip>>,
    ) -> (Flow, bool) {
        let PendingCycle {
            cycle_began,
            interval,
            now,
            failure,
            read_time,
            restart_timer,
        } = pending;
        // A tripped arm already holds where it tripped; faults, efforts and
        // temperatures still reach the clients in every state sample.
        if self.state.tripped.is_none()
            && let Some(Trip(reason)) = failure
        {
            self.trip(feedback, &reason, now);
        }
        let stop = self.state.core.stop_requested();
        if let Err(trip) = self.state.command(interval, feedback, stop, group, now) {
            self.trip(feedback, &trip.0, now);
            self.state.next_command.clone_from(&feedback.positions);
        }
        let command_start_time = self.started.elapsed().as_secs_f64();
        if let Err(failure) = self
            .plant
            .command(&self.state.next_command, interval)
            .and_then(|()| self.plant.heartbeat())
        {
            self.bus_failed(&failure, now);
            return (self.finished_or_continue(), false);
        }
        let write_time = self.started.elapsed().as_secs_f64();
        let mut record = self.trace.checkout();
        record.cycle = self.cycle_number;
        record.timings = self.timings;
        record.seconds = cycle_began;
        record.cycle_seconds = interval;
        record.command.clone_from(&self.state.next_command);
        record.positions.clone_from(&feedback.positions);
        record.velocities.clone_from(&feedback.velocities);
        record.efforts.clone_from(&feedback.efforts);
        record
            .temperatures_celsius
            .clone_from(&feedback.temperatures_celsius);
        record.voltages_volts.clone_from(&feedback.voltages_volts);
        record.feedback_fault.clone_from(&feedback.fault);
        record
            .environment_confirmations
            .clone_from(&feedback.environment_confirmations);
        record.read_time = read_time;
        record.command_start_time = command_start_time;
        record.write_time = write_time;
        self.trace.record(record);
        self.failed_cycles = 0;
        // What was sent is what the next cycle's checks compare against; the
        // old previous command's buffer is the next cycle's to fill.
        for ((velocity, next), previous) in self
            .state
            .commanded_velocities
            .iter_mut()
            .zip(&self.state.next_command)
            .zip(&self.state.previous_command)
        {
            *velocity = if interval > 0.0 {
                (next - previous) / interval
            } else {
                0.0
            };
        }
        std::mem::swap(
            &mut self.state.previous_command,
            &mut self.state.next_command,
        );
        self.publish(feedback, interval, true);
        (self.finished_or_continue(), restart_timer)
    }

    pub(super) fn finished_or_continue(&self) -> Flow {
        // A simulator that is gone ends the arm's run: nothing paces the loop any
        // more, and the closed socket tells the station the executor is not live.
        if self.plant_lost || self.state.core.shutdown_finished() {
            Flow::End
        } else {
            Flow::Continue
        }
    }

    pub(super) fn trip(&mut self, feedback: &Feedback, reason: &str, now: Now) {
        self.report.trip.get_or_insert_with(|| reason.to_owned());
        if let Some(link) = self.link {
            link.publish(Event::Tripped {
                reason: reason.to_owned(),
            });
        }
        self.state.hold_after_trip(reason, feedback, now);
    }

    /// Tell the clients this cycle's state, in a sample whose buffers the
    /// link's writer has finished with.
    pub(super) fn publish(&mut self, feedback: &Feedback, interval: f64, command_sent: bool) {
        if let (Some(link), Some(samples)) = (self.link, &self.samples) {
            let mut sample = samples.checkout();
            let core = &self.state.core;
            sample.cycle = self.cycle_number;
            sample.mode = core.mode();
            assign_text(&mut sample.active_command, core.active_command_id());
            assign_text(&mut sample.segment, core.segment());
            assign_numbers(
                &mut sample.hold_pose,
                core.poses().map(|poses| &poses.hold[..]),
            );
            assign_numbers(
                &mut sample.rest_pose,
                core.poses().map(|poses| &poses.rest[..]),
            );
            sample.positions.clone_from(&feedback.positions);
            assign_numbers(
                &mut sample.commanded_positions,
                command_sent.then_some(self.state.previous_command.as_slice()),
            );
            sample.velocities.clone_from(&feedback.velocities);
            sample.efforts.clone_from(&feedback.efforts);
            sample
                .temperatures_celsius
                .clone_from(&feedback.temperatures_celsius);
            sample.voltages_volts.clone_from(&feedback.voltages_volts);
            sample.gripper_angle = feedback.gripper_angle;
            sample.fault.clone_from(&feedback.fault);
            sample.recovery_issue = match &self.recovery {
                BusRecovery::AwaitingApproval(issue) => Some(issue.clone()),
                BusRecovery::Disabled
                | BusRecovery::Reconnecting
                | BusRecovery::VerifyingHold { .. } => None,
            };
            sample.cycle_seconds = interval;
            link.publish(Event::State(sample));
        }
        self.cycle_number += 1;
    }
}

/// Store `text` in `slot`, reusing the string already there.
fn assign_text(slot: &mut Option<String>, text: Option<&str>) {
    match (slot.as_mut(), text) {
        (Some(held), Some(text)) => {
            held.clear();
            held.push_str(text);
        }
        (_, text) => *slot = text.map(str::to_owned),
    }
}

/// Store `numbers` in `slot`, reusing the vector already there.
fn assign_numbers(slot: &mut Option<Vec<f64>>, numbers: Option<&[f64]>) {
    match (slot.as_mut(), numbers) {
        (Some(held), Some(numbers)) => {
            held.clear();
            held.extend_from_slice(numbers);
        }
        (_, numbers) => *slot = numbers.map(<[f64]>::to_vec),
    }
}

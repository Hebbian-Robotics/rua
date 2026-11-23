use std::time::{Duration, Instant};

use super::activity::Activity;
use super::activity::Loop;
use super::recovery::{RecoveryHold, assess_recovery_hold, recovery_positions};
use super::trajectory::GroupReference;
use super::{LONGEST_RECORDED_CYCLE_MICROSECONDS, Report};
use crate::arm::{Job, Now};
use crate::config::ControllerConfig;
use crate::control_link::{Link, SampleRecycler};
use crate::gripper::GripperState;
use crate::handoff::ArmInbox;
use crate::limits::Trip;
use crate::plant::{Feedback, Plant, Reconnection};
use crate::trace::TraceWriter;
use hebbian_contracts::ArmRecoveryIssue;
use hebbian_contracts::{ArmMode, CommandOutcome, Event, GripperResult};
const BUS_FAILURE_GRACE_SECONDS: f64 = 0.5;
// SAFETY: A cycle longer than this many periods is fed to the generators as this
// many, so a late cycle does not become a jump.
const LONGEST_STEP_PERIODS: f64 = 2.0;

pub(super) enum BusRecovery {
    Disabled,
    Reconnecting,
    VerifyingHold { feedback: Feedback, since: Instant },
    AwaitingApproval(ArmRecoveryIssue),
}

pub(super) struct PendingCycle {
    pub(super) cycle_began: f64,
    pub(super) interval: f64,
    pub(super) now: Now,
    pub(super) failure: Option<Trip>,
    pub(super) read_time: f64,
    pub(super) restart_timer: bool,
}

pub(super) enum Begun {
    Done(Flow, bool),
    Pending(PendingCycle),
}

// INVARIANT: Only a successful validated read may replace the retained sample.
enum CycleReading {
    Trusted(Begun),
    Unavailable(Flow),
}

#[derive(Debug)]
struct GripperWrite {
    angle: f64,
    completed: Option<GripperResult>,
}

#[derive(Debug)]
pub(super) enum GripperCycleFailure {
    Control(anyhow::Error),
    Plant(anyhow::Error),
}

pub(super) enum Flow {
    Continue,
    End,
}

pub(super) struct Runner<'a, P: Plant> {
    pub(super) name: String,
    pub(super) plant: &'a mut P,
    pub(super) link: &'a Link,
    pub(super) inbox: ArmInbox,
    pub(super) samples: SampleRecycler,
    pub(super) feedback: Feedback,
    // INVARIANT: Reused read buffer; failed reads never replace the retained trusted sample.
    pub(super) feedback_scratch: Feedback,
    pub(super) config: &'a ControllerConfig,
    pub(super) state: Loop<'a>,
    pub(super) trace: TraceWriter,
    pub(super) timings: crate::trace::CycleTimings,
    pub(super) report: Report,
    pub(super) started: Instant,
    pub(super) cycle_number: u64,
    pub(super) plant_lost: bool,
    pub(super) failed_cycles: u32,
    pub(super) last_reconnect_failure: Option<String>,
    pub(super) recovery: BusRecovery,
}

impl<P: Plant> Runner<'_, P> {
    pub(super) fn plant_failed(&mut self, failure: &anyhow::Error, now: Now) {
        self.plant_lost = self.plant.pacing().is_set_by_replies();
        let reason = format!("{failure:#}");
        self.report.trip.get_or_insert_with(|| reason.clone());
        if self.state.powered() {
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

    // SAFETY: Interrupt the active motion without issuing a torque-off command.
    fn bus_lost(&mut self, failure: &anyhow::Error, now: Now) {
        if self.plant.pacing().is_set_by_replies() {
            self.plant_failed(failure, now);
            return;
        }
        let reason = format!("{failure:#}");
        self.report.trip.get_or_insert_with(|| reason.clone());
        self.recovery = if self.state.powered() {
            BusRecovery::Reconnecting
        } else {
            BusRecovery::Disabled
        };
        self.state.power_down();
        tracing::warn!(arm = %self.name, %reason, "Bus disconnected; retrying");
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
        if matches!(self.recovery, BusRecovery::VerifyingHold { .. }) {
            self.verify_recovery_hold(feedback, now);
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
                    tracing::warn!(arm = %self.name, %reason, "Bus still disconnected");
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
        if let Err(refusal) = recovery_positions(self.config, &observation.feedback) {
            self.recovery = BusRecovery::AwaitingApproval(ArmRecoveryIssue {
                motors_without_torque: Vec::new(),
                detail: refusal.to_string(),
            });
            return;
        }
        match self.plant.hold_recovery() {
            Ok(observation) => {
                self.recovery = match recovery_positions(self.config, &observation.feedback) {
                    Ok(_) => BusRecovery::VerifyingHold {
                        feedback: observation.feedback,
                        since: Instant::now(),
                    },
                    Err(refusal) => BusRecovery::AwaitingApproval(ArmRecoveryIssue {
                        motors_without_torque: Vec::new(),
                        detail: refusal.to_string(),
                    }),
                };
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
            recovery_positions(self.config, &observation.feedback)?;
            let drift_limit = self
                .config
                .enable_checks
                .as_ref()
                .map_or(self.config.motion.settled_error_radians, |checks| {
                    checks.preflight_drift_limit_radians
                });
            let feedback = self.plant.enable_recovery(drift_limit)?.feedback;
            recovery_positions(self.config, &feedback)?;
            Ok(feedback)
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

    fn verify_recovery_hold(&mut self, feedback: &mut Feedback, now: Now) {
        let BusRecovery::VerifyingHold {
            feedback: held,
            since,
        } = &self.recovery
        else {
            return;
        };
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
            Ok(observation) => {
                match assess_recovery_hold(self.config, held, &observation, since.elapsed()) {
                    RecoveryHold::Waiting => {}
                    RecoveryHold::Verified => {
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
                            now,
                            &feedback.positions,
                            tool_pose,
                            feedback.fault.as_deref(),
                        );
                        self.failed_cycles = 0;
                        self.last_reconnect_failure = None;
                        self.recovery = BusRecovery::Disabled;
                        tracing::info!(arm = %self.name, "Recovered at a measured hold");
                    }
                    RecoveryHold::Refused(issue) => {
                        self.state
                            .arm()
                            .failed("The arm did not remain at the recovery hold", now);
                        self.recovery = BusRecovery::AwaitingApproval(issue);
                    }
                }
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

    // SAFETY: A bus failure: an enabled arm's servos keep holding their last goal,
    // so nothing new is commanded and the bus is tried again next cycle; only
    // a failure that lasts the grace period counts as the bus lost.
    pub(super) fn bus_failed(&mut self, failure: &anyhow::Error, now: Now) {
        self.failed_cycles += 1;
        let patient = !self.plant.pacing().is_set_by_replies()
            && f64::from(self.failed_cycles)
                < BUS_FAILURE_GRACE_SECONDS * self.config.frequency_hertz;
        if !patient {
            self.bus_lost(failure, now);
        } else if let Err(heartbeat_failure) = self.plant.heartbeat() {
            // INVARIANT: The heartbeat is not on the bus, and the guard must keep hearing it:
            // a guard that is gone leaves the arm unwatched, so patience ends.
            self.plant_failed(&heartbeat_failure, now);
        }
    }

    pub(super) fn drive_gripper(
        &mut self,
        feedback: &Feedback,
        interval: f64,
        now: Now,
    ) -> Result<(), GripperCycleFailure> {
        if let Some(write) = self
            .prepare_gripper_write(feedback, interval, now)
            .map_err(GripperCycleFailure::Control)?
        {
            let write_started = Instant::now();
            self.plant
                .command_gripper(write.angle)
                .map_err(GripperCycleFailure::Plant)?;
            self.timings.gripper_write_seconds = write_started.elapsed().as_secs_f64();
            if let Some(result) = write.completed
                && let Some(command_id) = self.state.gripper_command.take()
            {
                self.state.arm().finish_gripper(
                    &command_id,
                    CommandOutcome::Completed,
                    Some(result.describe()),
                    Some(result),
                    now,
                );
            }
        }
        Ok(())
    }

    fn prepare_gripper_write(
        &mut self,
        feedback: &Feedback,
        interval: f64,
        now: Now,
    ) -> anyhow::Result<Option<GripperWrite>> {
        let (Some(measured), Some(_)) = (feedback.gripper_angle, self.state.gripper.as_ref())
        else {
            if self.state.powered() && self.config.gripper.is_some() {
                return Err(anyhow::anyhow!(
                    "Missing measurement or controller for the configured gripper"
                ));
            }
            return Ok(None);
        };
        if self.state.arm().take_gripper_stop() {
            // INVARIANT: The record already ended stopped; the jaw holds where it is.
            self.state.gripper_command = None;
            if let Some(jaw) = &mut self.state.gripper {
                jaw.hold(measured);
            }
        }
        let new_target = self.state.arm().take_gripper_target();
        if let Some((command_id, gripper_target)) = new_target {
            if let Some(jaw) = &mut self.state.gripper {
                jaw.set_target(
                    gripper_target.angle.radians(),
                    gripper_target.speed.radians_per_second(),
                    measured,
                )?;
            }
            self.state.gripper_command = Some(command_id);
        }
        let Some(jaw) = &mut self.state.gripper else {
            return Ok(None);
        };
        let proposed_command = jaw.step(measured, interval)?;
        let target = jaw.target();
        // WHY: Check only this cycle's smoothed travel, rather than scanning the
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
                        None,
                        now,
                    );
                }
                return Ok(None);
            }
        };
        let jaw = self.state.gripper.as_mut().ok_or_else(|| {
            anyhow::anyhow!("The active gripper controller disappeared during its cycle")
        })?;
        let clipped = (command - proposed_command).abs() > 1e-8;
        if clipped {
            jaw.hold(command);
        }
        let state = jaw.state();
        let completed = match (state, clipped) {
            (GripperState::Moving, _) => None,
            (GripperState::Reached, false) => Some(GripperResult::Reached {
                measured_angle_radians: measured,
            }),
            _ => Some(GripperResult::StoppedShort {
                measured_angle_radians: measured,
                target_angle_radians: target,
            }),
        };
        Ok(Some(GripperWrite {
            angle: command,
            completed,
        }))
    }

    fn resolve_gripper_failure(
        &mut self,
        failure: GripperCycleFailure,
        feedback: &Feedback,
        now: Now,
    ) -> anyhow::Result<()> {
        match failure {
            GripperCycleFailure::Plant(failure) => Err(failure),
            GripperCycleFailure::Control(failure) => {
                let detail = format!("Gripper control failed: {failure:#}");
                self.state.core.fail_gripper(&detail, now);
                self.state.gripper_command = None;
                self.trip(feedback, Trip::Gripper(detail), now);
                if let Some(measured) = feedback.gripper_angle {
                    if let Some(jaw) = &mut self.state.gripper {
                        jaw.hold(measured);
                    }
                    self.plant.command_gripper(measured)?;
                }
                Ok(())
            }
        }
    }

    // INVARIANT: Step each grouped trajectory between the read and command halves, after every member has supplied this cycle’s reading.
    // WHY: Enabling may take longer than a cycle; restart the cycle timer after a long admission.
    pub(super) fn dispatch_next_command(&mut self, now: Now) -> bool {
        let (job, mode) = {
            let arm = self.state.arm();
            (arm.next_job(), arm.mode())
        };
        let Some(job) = job else {
            return self.shutdown_step(now);
        };
        match job {
            Job::ApproveRecovery { .. } => {
                self.state
                    .arm()
                    .failed("The arm is not waiting for recovery approval", now);
                false
            }
            Job::Enable { .. } => {
                if mode.accepts_enable() {
                    self.enable(now);
                } else {
                    self.state.arm().failed("The arm is already enabled", now);
                }
                true
            }
            Job::Release { .. } => {
                if self.state.powered() {
                    self.release(now);
                } else {
                    self.state.arm().failed(
                        "The arm is not enabled; send enable, then repeat the command",
                        now,
                    );
                }
                true
            }
            Job::Move { .. } | Job::Stream { .. } | Job::Reach { .. } if !self.state.powered() => {
                self.state.arm().failed(
                    "The arm is not enabled; send enable, then repeat the command",
                    now,
                );
                false
            }
            Job::Move { .. } | Job::Stream { .. } | Job::Reach { .. }
                if self.state.tripped.is_some() =>
            {
                let reason = self
                    .state
                    .tripped
                    .as_ref()
                    .map_or("", Trip::detail)
                    .to_owned();
                self.state
                    .arm()
                    .failed(&format!("The arm is tripped: {reason}"), now);
                false
            }
            Job::Move {
                plan,
                group: Some(group),
                ..
            } => {
                self.state.start_group_member(plan, group);
                self.state.arm().begin_moving();
                false
            }
            Job::Move { plan, .. } => {
                match self.state.start_motion(plan) {
                    Ok(()) => self.state.arm().begin_moving(),
                    Err(reason) => self.state.arm().failed(&format!("{reason:#}"), now),
                }
                false
            }
            Job::Reach { goal, .. } => {
                match self.state.start_reach(goal) {
                    Ok(()) => self.state.arm().begin_moving(),
                    Err(reason) => self.state.arm().failed(&format!("{reason:#}"), now),
                }
                false
            }
            Job::Stream { command_id } => {
                self.state.start_stream(command_id);
                self.state.arm().begin_moving();
                false
            }
        }
    }

    pub(super) fn begin_cycle(
        &mut self,
        cycle: Duration,
        deadline_missed: Option<String>,
    ) -> Begun {
        let mut feedback = std::mem::take(&mut self.feedback_scratch);
        match self.begin_cycle_with(&mut feedback, cycle, deadline_missed) {
            CycleReading::Trusted(begun) => {
                self.feedback_scratch = std::mem::replace(&mut self.feedback, feedback);
                begun
            }
            CycleReading::Unavailable(flow) => {
                self.feedback_scratch = feedback;
                Begun::Done(flow, false)
            }
        }
    }

    fn begin_cycle_with(
        &mut self,
        feedback: &mut Feedback,
        cycle: Duration,
        deadline_missed: Option<String>,
    ) -> CycleReading {
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
            feedback.clone_from(&self.feedback);
            self.reconnect_step(feedback, now);
            // INVARIANT: The last reading stands; the sample's mode says it is stale.
            if feedback.measured_positions(self.config).is_ok() {
                self.publish(feedback, interval, false);
            }
            return CycleReading::Trusted(Begun::Done(self.finished_or_continue(), false));
        }
        let reading_began = Instant::now();
        let reading = self
            .plant
            .read_feedback_into(feedback)
            .and_then(|()| feedback.measured_positions(self.config));
        self.timings.feedback_read_seconds = reading_began.elapsed().as_secs_f64();
        let positions = match reading {
            Ok(positions) => positions,
            Err(failure) => {
                self.bus_failed(&failure, now);
                if matches!(
                    self.state.core.mode(),
                    ArmMode::Disconnected | ArmMode::Faulted
                ) {
                    // INVARIANT: Publish the transition, never the unspecified failed-read buffer.
                    let retained = std::mem::take(&mut self.feedback);
                    if retained.measured_positions(self.config).is_ok() {
                        self.publish(&retained, interval, false);
                    }
                    self.feedback = retained;
                }
                return CycleReading::Unavailable(self.finished_or_continue());
            }
        };
        let read_time = self.started.elapsed().as_secs_f64();
        let tool_pose_began = Instant::now();
        let tool_pose = self
            .state
            .tool
            .as_ref()
            .and_then(|tool| tool.pose(feedback));
        self.timings.tool_pose_seconds = tool_pose_began.elapsed().as_secs_f64();
        self.state.core.observe(
            now,
            positions.as_slice(),
            tool_pose,
            feedback.fault.as_deref(),
        );
        let failure = self
            .state
            .cycle_failure(feedback, interval, deadline_missed);
        if failure.is_none() {
            self.state.tripped = None;
        }
        let start_next_began = Instant::now();
        let restart_timer = if matches!(self.state.activity, Activity::Idle) {
            self.dispatch_next_command(now)
        } else {
            false
        };
        self.timings.start_next_seconds = start_next_began.elapsed().as_secs_f64();
        let gripper_began = Instant::now();
        if let Err(failure) = self.drive_gripper(feedback, interval, now)
            && let Err(transport_failure) = self.resolve_gripper_failure(failure, feedback, now)
        {
            self.bus_failed(&transport_failure, now);
            return CycleReading::Trusted(Begun::Done(self.finished_or_continue(), false));
        }
        self.timings.gripper_seconds = gripper_began.elapsed().as_secs_f64();
        if !self.state.powered() {
            self.failed_cycles = 0;
            // CONSTRAINT: A paced plant (a simulator) moves time on only when told to.
            if let Err(failure) = self.plant.advance(interval) {
                self.plant_failed(&failure, now);
                return CycleReading::Trusted(Begun::Done(self.finished_or_continue(), false));
            }
            self.publish(feedback, interval, false);
            return CycleReading::Trusted(Begun::Done(self.finished_or_continue(), restart_timer));
        }
        CycleReading::Trusted(Begun::Pending(PendingCycle {
            cycle_began,
            interval,
            now,
            failure,
            read_time,
            restart_timer,
        }))
    }

    pub(super) fn finish_cycle(
        &mut self,
        pending: PendingCycle,
        group: Option<Result<&GroupReference, &Trip>>,
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
        group: Option<Result<&GroupReference, &Trip>>,
    ) -> (Flow, bool) {
        let PendingCycle {
            cycle_began,
            interval,
            now,
            failure,
            read_time,
            restart_timer,
        } = pending;
        // INVARIANT: A tripped arm already holds where it tripped; faults, efforts and
        // temperatures still reach the clients in every state sample.
        if self.state.tripped.is_none()
            && let Some(trip) = failure
        {
            self.trip(feedback, trip, now);
        }
        let stop = self.state.core.stop_requested();
        let computation_began = Instant::now();
        let computed = self.state.command(interval, feedback, stop, group, now);
        self.timings.command_compute_seconds = computation_began.elapsed().as_secs_f64();
        self.timings.policy_inference_seconds = self
            .state
            .policy
            .as_ref()
            .map_or(0.0, crate::policy::PolicyRuntime::inference_seconds);
        if let Err(trip) = computed {
            self.trip(feedback, trip, now);
            self.state.next_command.clone_from(&feedback.positions);
        }
        let command_start_time = self.started.elapsed().as_secs_f64();
        let write_began = Instant::now();
        if let Err(failure) = self
            .plant
            .command(&self.state.next_command, interval)
            .and_then(|()| self.plant.heartbeat())
        {
            self.bus_failed(&failure, now);
            return (self.finished_or_continue(), false);
        }
        self.timings.command_write_seconds = write_began.elapsed().as_secs_f64();
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
        // INVARIANT: What was sent is what the next cycle's checks compare against; the
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
        // CONSTRAINT: A simulator that is gone ends the arm's run: nothing paces the loop any
        // more, and the closed socket tells the station the controller is not live.
        if self.plant_lost || self.state.core.shutdown_finished() {
            Flow::End
        } else {
            Flow::Continue
        }
    }

    pub(super) fn trip(&mut self, feedback: &Feedback, trip: Trip, now: Now) {
        let reason = trip.detail();
        self.report.trip.get_or_insert_with(|| reason.to_owned());
        self.link.publish(Event::Tripped {
            reason: reason.to_owned(),
        });
        self.state.hold_after_trip(trip, feedback, now);
    }

    pub(super) fn publish(&mut self, feedback: &Feedback, interval: f64, command_sent: bool) {
        let mut sample = self.samples.checkout();
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
        assign_text(
            &mut sample.active_trip,
            self.state.tripped.as_ref().map(Trip::detail),
        );
        sample.recovery_issue = match &self.recovery {
            BusRecovery::AwaitingApproval(issue) => Some(issue.clone()),
            BusRecovery::Disabled
            | BusRecovery::Reconnecting
            | BusRecovery::VerifyingHold { .. } => None,
        };
        sample.cycle_seconds = interval;
        self.link.publish(Event::State(sample));
        self.cycle_number += 1;
    }
}

fn assign_text(slot: &mut Option<String>, text: Option<&str>) {
    match (slot.as_mut(), text) {
        (Some(held), Some(text)) => {
            held.clear();
            held.push_str(text);
        }
        (_, text) => *slot = text.map(str::to_owned),
    }
}

fn assign_numbers(slot: &mut Option<Vec<f64>>, numbers: Option<&[f64]>) {
    match (slot.as_mut(), numbers) {
        (Some(held), Some(numbers)) => {
            held.clear();
            held.extend_from_slice(numbers);
        }
        (_, numbers) => *slot = numbers.map(<[f64]>::to_vec),
    }
}

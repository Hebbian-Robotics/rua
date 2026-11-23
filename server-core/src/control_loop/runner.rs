use std::path::Path;
use std::time::{Duration, Instant};

use super::activity::Activity;
use super::activity::{Loop, initialize_loop};
use super::cycle::{CycleOutputs, GripperEffect};
use super::learned::{MotionPlan, PolicyRequest, PolicySession, policy_failure};
use super::recovery::{RecoveryHold, assess_recovery_hold, recovery_positions};
use super::trajectory::{GroupReference, GroupWriteAttempt, MotionEffects, MotionOutput};
use super::{LONGEST_RECORDED_CYCLE_MICROSECONDS, Report};
use crate::arm::{ArmCore, Job, Now};
use crate::config::ControllerConfig;
use crate::control_link::{Link, SampleRecycler};
use crate::handoff::ArmInbox;
use crate::limits::Trip;
use crate::plant::{Feedback, Pacing, Plant, Reconnection};
use crate::trace::TraceWriter;
use hebbian_contracts::ArmRecoveryIssue;
use hebbian_contracts::{ArmMode, CommandOutcome, LoggedEvent};
use power::StartupHold;
#[path = "power.rs"]
mod power;
#[cfg(test)]
#[path = "runner_tests.rs"]
mod runner_tests;

// SAFETY: Only the runner creates a permit. Adapters borrow it for an actuation call.
pub struct PlantWritePermit {
    _private: (),
}

const WRITE_PERMIT: PlantWritePermit = PlantWritePermit { _private: () };

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

pub(super) struct FinishedCycle {
    pub(super) flow: Flow,
    pub(super) restart_timer: bool,
    pub(super) group_write: Option<GroupWrite>,
}

pub(super) struct GroupWrite {
    effects: MotionEffects,
    now: Now,
    interval: f64,
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

enum SetpointFailure {
    Gripper(anyhow::Error),
    Arm(anyhow::Error),
}

pub(super) enum Flow {
    Continue,
    End,
}

pub(super) struct Runner<'a, P: Plant> {
    pub(super) name: String,
    plant: &'a mut P,
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
    pub(super) startup_hold: Option<StartupHold>,
}

pub(super) fn runner_for<'a, P: Plant>(
    name: &str,
    plant: &'a mut P,
    link: &'a Link,
    config: &'a ControllerConfig,
    trace_directory: &Path,
) -> Result<Runner<'a, P>, Box<Report>> {
    let mut report = Report::empty();
    let trace = match TraceWriter::open(trace_directory) {
        Ok(trace) => trace,
        Err(reason) => {
            report.trip = Some(format!("{reason:#}"));
            return Err(Box::new(report));
        }
    };
    let core = ArmCore::new(config.arm_limits(), link.log_publisher());
    let initialized = plant.read_feedback().and_then(|feedback| {
        feedback.measured_positions(config)?;
        let state = initialize_loop(config, core, &feedback)?;
        Ok((state, feedback))
    });
    let (state, feedback) = match initialized {
        Ok(initialized) => initialized,
        Err(reason) => {
            report.trip = Some(format!("{reason:#}"));
            return Err(Box::new(report));
        }
    };
    let Some(loop_end) = link.take_loop_end() else {
        report.trip = Some(format!("The control link for {name} is already in use"));
        return Err(Box::new(report));
    };
    Ok(Runner {
        name: name.to_owned(),
        plant,
        link,
        inbox: loop_end.inbox,
        samples: loop_end.samples,
        feedback,
        feedback_scratch: Feedback::default(),
        config,
        state,
        trace,
        timings: crate::trace::CycleTimings::default(),
        report,
        started: Instant::now(),
        cycle_number: 0,
        plant_lost: false,
        failed_cycles: 0,
        last_reconnect_failure: None,
        recovery: BusRecovery::Disabled,
        startup_hold: None,
    })
}

impl<P: Plant> Runner<'_, P> {
    pub(super) fn pacing(&self) -> Pacing {
        self.plant.pacing()
    }

    pub(super) fn advance_plant(&mut self, interval: f64) -> anyhow::Result<()> {
        self.plant.advance(&WRITE_PERMIT, interval)
    }

    pub(super) fn read_plant_feedback(&mut self) -> anyhow::Result<Feedback> {
        self.plant.read_feedback()
    }

    pub(super) fn plant_failed(&mut self, failure: &anyhow::Error, now: Now) {
        self.plant_lost = self.plant.pacing().is_set_by_replies();
        let reason = format!("{failure:#}");
        self.report.trip.get_or_insert_with(|| reason.clone());
        if self.state.powered() {
            let cleanup_errors: Vec<String> = self
                .plant
                .release(&WRITE_PERMIT)
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
        match self.plant.hold_recovery(&WRITE_PERMIT) {
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
            let feedback = self
                .plant
                .enable_recovery(&WRITE_PERMIT, drift_limit)?
                .feedback;
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
                    self.start_enable(now);
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
        if let Some(hold) = self.startup_hold.take() {
            feedback.clone_from(&self.feedback);
            self.progress_enable(hold, feedback, now);
            return self.finish_inactive_cycle(feedback, interval, now, false);
        }
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
        if let Err(failure) = reading {
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
        let read_time = self.started.elapsed().as_secs_f64();
        self.observe_feedback(feedback, now);

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
        if let Some(hold) = self.startup_hold.take() {
            self.progress_enable(hold, feedback, now);
            return self.finish_inactive_cycle(feedback, interval, now, restart_timer);
        }
        if !self.state.powered() {
            return self.finish_inactive_cycle(feedback, interval, now, restart_timer);
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

    pub(super) fn observe_feedback(&mut self, feedback: &Feedback, now: Now) {
        let tool_pose_began = Instant::now();
        let tool_pose = self
            .state
            .tool
            .as_ref()
            .and_then(|tool| tool.pose(feedback));
        self.timings.tool_pose_seconds = tool_pose_began.elapsed().as_secs_f64();
        self.state.core.observe(
            now,
            &feedback.positions,
            tool_pose,
            feedback.fault.as_deref(),
        );
    }

    fn finish_inactive_cycle(
        &mut self,
        feedback: &Feedback,
        interval: f64,
        now: Now,
        restart_timer: bool,
    ) -> CycleReading {
        self.failed_cycles = 0;
        // CONSTRAINT: A paced plant (a simulator) moves time on only when told to.
        if let Err(failure) = self.advance_plant(interval) {
            if self.startup_hold.is_some() {
                self.fail_enable(&format!("{failure:#}"), now);
            }
            self.plant_failed(&failure, now);
            return CycleReading::Trusted(Begun::Done(self.finished_or_continue(), false));
        }
        self.publish(feedback, interval, false);
        CycleReading::Trusted(Begun::Done(self.finished_or_continue(), restart_timer))
    }

    pub(super) fn finish_cycle(
        &mut self,
        pending: PendingCycle,
        group: Option<Result<&GroupReference, &Trip>>,
    ) -> FinishedCycle {
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
    ) -> FinishedCycle {
        let PendingCycle {
            cycle_began,
            interval,
            now,
            failure,
            read_time,
            restart_timer,
        } = pending;
        let gripper_began = Instant::now();
        let gripper = self.state.plan_gripper(feedback, interval, failure);
        self.timings.gripper_seconds = gripper_began.elapsed().as_secs_f64();
        let computation_began = Instant::now();
        if let Some(runtime) = &mut self.state.policy {
            runtime.begin_cycle();
        }
        let motion = self.state.plan_motion(interval, feedback, group, now);
        let motion = motion.and_then(|plan| match plan {
            MotionPlan::ReferenceReady(effects) => Ok(effects),
            MotionPlan::Infer(request) => self.evaluate_policy(&request, feedback),
        });
        let CycleOutputs {
            motion,
            gripper,
            trip,
        } = self
            .state
            .finish_cycle_plan(motion, gripper, feedback, interval);
        self.timings.command_compute_seconds = computation_began.elapsed().as_secs_f64();
        self.timings.policy_inference_seconds = self
            .state
            .policy
            .as_ref()
            .map_or(0.0, crate::policy::PolicyRuntime::inference_seconds);
        let command_start_time = self.started.elapsed().as_secs_f64();
        let written = self.write_setpoints(&gripper, interval);
        self.publish_trip(trip, feedback, now);
        self.commit_gripper(
            gripper,
            !matches!(written, Err(SetpointFailure::Gripper(_))),
            now,
        );
        if let Err(SetpointFailure::Arm(failure) | SetpointFailure::Gripper(failure)) = written {
            self.bus_failed(&failure, now);
            return FinishedCycle {
                flow: self.finished_or_continue(),
                restart_timer: false,
                group_write: None,
            };
        }
        let write_time = self.started.elapsed().as_secs_f64();
        let repeated_group_setpoint = matches!(
            motion,
            MotionOutput::Group {
                attempt: GroupWriteAttempt::Retry,
                ..
            }
        );
        let group_write = match motion {
            MotionOutput::None => None,
            MotionOutput::Arm(effects) => {
                self.state.commit_motion_effects(effects, now);
                None
            }
            MotionOutput::Group { effects, .. } => Some(GroupWrite {
                effects,
                now,
                interval,
            }),
        };
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
        self.accept_joint_setpoint(interval, repeated_group_setpoint);
        if group_write.is_none() {
            self.publish(feedback, interval, true);
        }
        FinishedCycle {
            flow: self.finished_or_continue(),
            restart_timer,
            group_write,
        }
    }

    fn evaluate_policy(
        &mut self,
        request: &PolicyRequest,
        feedback: &Feedback,
    ) -> Result<MotionOutput, Trip> {
        let runtime =
            self.state.policy.as_mut().ok_or_else(|| {
                Trip::Policy("A pose goal arrived but no policy is loaded".into())
            })?;
        if matches!(request.session, PolicySession::Reset) {
            runtime.reset();
        }
        let positions = runtime
            .joint_goal(self.config, feedback, &request.target)
            .map_err(|error| policy_failure(&error))?;
        Ok(self.state.adopt_policy_target(&positions))
    }

    fn accept_joint_setpoint(&mut self, interval: f64, repeated_group_setpoint: bool) {
        // INVARIANT: What was sent is what the next cycle's checks compare against; the
        // old previous command's buffer is the next cycle's to fill.
        for ((velocity, next), previous) in self
            .state
            .commanded_velocities
            .iter_mut()
            .zip(&self.state.next_command)
            .zip(&self.state.previous_command)
        {
            // WHY: A repeated setpoint can still be ahead of the moving joint.
            // Keep its tracking allowance until the group can advance again.
            if repeated_group_setpoint && next.to_bits() == previous.to_bits() {
                continue;
            }
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
    }

    pub(super) fn finish_group_write(&mut self, write: GroupWrite, confirmed: bool) {
        if confirmed {
            self.state.commit_motion_effects(write.effects, write.now);
        }
        let feedback = std::mem::take(&mut self.feedback);
        self.publish(&feedback, write.interval, true);
        self.feedback = feedback;
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

    fn write_setpoints(
        &mut self,
        gripper: &GripperEffect,
        interval: f64,
    ) -> Result<(), SetpointFailure> {
        // CONSTRAINT: The simulator includes its queued jaw target in the joint step.
        if let Some(angle) = gripper.setpoint() {
            let gripper_started = Instant::now();
            let written = self.plant.command_gripper(&WRITE_PERMIT, angle);
            self.timings.gripper_write_seconds = gripper_started.elapsed().as_secs_f64();
            written.map_err(SetpointFailure::Gripper)?;
        }
        let write_started = Instant::now();
        let written = self
            .plant
            .command(&WRITE_PERMIT, &self.state.next_command, interval)
            .and_then(|()| self.plant.heartbeat());
        self.timings.command_write_seconds = write_started.elapsed().as_secs_f64();
        written.map_err(SetpointFailure::Arm)
    }

    fn commit_gripper(&mut self, gripper: GripperEffect, confirmed: bool, now: Now) {
        match gripper {
            GripperEffect::Write(write) if confirmed => {
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
            GripperEffect::Refused(detail) | GripperEffect::Failed { detail, .. } => {
                self.state.gripper_command = None;
                self.state.core.fail_gripper(&detail, now);
            }
            GripperEffect::Idle | GripperEffect::Write(_) => {}
        }
    }

    fn publish_trip(&mut self, trip: Option<String>, feedback: &Feedback, now: Now) {
        let Some(reason) = trip else { return };
        self.report.trip.get_or_insert_with(|| reason.clone());
        self.link.publish_log(LoggedEvent::tripped(reason.clone()));
        self.state.arm().motion_ended(
            CommandOutcome::Tripped,
            Some(reason),
            &feedback.positions,
            now,
        );
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
        self.link.publish_sample(sample);
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

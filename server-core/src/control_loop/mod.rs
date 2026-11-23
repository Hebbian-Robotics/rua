mod activity;
mod cycle;
mod group;
mod guards;
mod learned;
mod recovery;
mod runner;
mod tool_pose;
mod trajectory;

#[cfg(test)]
mod tests;

use group::{ActiveGroup, confirm_group_writes, form_groups, settle_groups, step_groups};
pub(crate) use guards::{
    build_guard, build_self_collision, clamp_setpoint, guard_command, load_shared_robot,
};
pub use runner::PlantWritePermit;
use runner::{Begun, Flow, PendingCycle, Runner, runner_for};

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, ensure};
use hdrhistogram::Histogram;

use crate::arm::{ArmCore, Now};
use crate::config::ControllerConfig;
use crate::control_link::Link;
use crate::cycle::CycleTimer;
use crate::handoff::{ArmRequest, MAXIMUM_REQUESTS_PER_CYCLE};
use crate::plant::{Pacing, Plant};
use crate::trace::TraceSummary;
use hebbian_contracts::Command;

// CONSTRAINT: Cycle times are recorded in microseconds up to a minute, to three
// significant digits: a fixed few kilobytes however long the loop runs.
pub(super) const LONGEST_RECORDED_CYCLE_MICROSECONDS: u64 = 60_000_000;
const CYCLE_TIME_SIGNIFICANT_DIGITS: u8 = 3;
pub(crate) const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(20);

pub struct Report {
    pub cycle_microseconds: Histogram<u64>,
    pub trip: Option<String>,
    pub overruns: u64,
    pub trace: Option<TraceSummary>,
}

impl Report {
    #[expect(
        clippy::expect_used,
        reason = "Histogram bounds and precision are fixed valid constants."
    )]
    fn empty() -> Self {
        Self {
            cycle_microseconds: Histogram::new_with_bounds(
                1,
                LONGEST_RECORDED_CYCLE_MICROSECONDS,
                CYCLE_TIME_SIGNIFICANT_DIGITS,
            )
            .expect("the histogram bounds are valid"),
            trip: None,
            overruns: 0,
            trace: None,
        }
    }
}

fn host_load_averages() -> Option<[f64; 3]> {
    let mut averages = [0.0_f64; 3];
    // SAFETY: getloadavg writes at most 3 doubles into the buffer it is given.
    let written = unsafe { libc::getloadavg(averages.as_mut_ptr(), 3) };
    (written == 3).then_some(averages)
}

fn deadline_missed_message(cycle: Duration, load: Option<[f64; 3]>) -> String {
    match load {
        Some([one, five, fifteen]) => format!(
            "Loop deadline missed: {cycle:?} (host load averages 1/5/15 min: {one:.2} {five:.2} {fifteen:.2})"
        ),
        None => format!("Loop deadline missed: {cycle:?}"),
    }
}

#[expect(
    clippy::expect_used,
    reason = "run_runners returns one report per runner; this call supplies exactly one runner."
)]
pub fn run(
    plant: &mut impl Plant,
    link: &Link,
    config: &ControllerConfig,
    run_for: Option<Duration>,
    shutdown_requested: &AtomicBool,
) -> Report {
    match runner_for(
        "arm",
        plant,
        link,
        config,
        Path::new(&config.trace_directory),
    ) {
        Ok(runner) => run_runners(vec![runner], run_for, shutdown_requested)
            .pop()
            .expect("one arm, one report"),
        Err(report) => *report,
    }
}

pub struct ArmSetup<P: Plant> {
    pub name: String,
    pub config: ControllerConfig,
    plant: P,
    pub link: Link,
}

impl<P: Plant> ArmSetup<P> {
    pub fn new(name: String, config: ControllerConfig, plant: P, link: Link) -> Self {
        Self {
            name,
            config,
            plant,
            link,
        }
    }

    pub fn plant(&self) -> &P {
        &self.plant
    }

    pub fn verify_disabled(&mut self, verify_torque: bool) -> anyhow::Result<()> {
        self.plant
            .preflight()
            .with_context(|| format!("{}: preflight failed", self.name))?;
        let feedback = if verify_torque {
            let observation = self
                .plant
                .observe_recovery()
                .with_context(|| format!("{}: cannot confirm torque", self.name))?;
            let expected_motors =
                self.config.joint_count + usize::from(self.config.gripper.is_some());
            ensure!(
                observation.motor_torque.len() == expected_motors
                    && observation.motor_torque.iter().all(|(_, enabled)| !enabled),
                "{}: motor torque is not confirmed off",
                self.name
            );
            if let Some(issue) = observation.health_issue {
                anyhow::bail!("{}: {issue}", self.name);
            }
            observation.feedback
        } else {
            self.plant
                .read_feedback()
                .with_context(|| format!("{}: cannot read feedback", self.name))?
        };
        feedback
            .measured_positions(&self.config)
            .with_context(|| format!("{}: invalid feedback", self.name))?;
        if let Some(fault) = feedback.fault {
            anyhow::bail!("{}: {fault}", self.name);
        }
        Ok(())
    }
}

// INVARIANT: Read every arm before stepping shared group trajectories once, then command every arm. All members must share a frequency and compatible pacing.
pub fn run_arms<P: Plant>(
    arms: &mut [ArmSetup<P>],
    run_for: Option<Duration>,
    shutdown_requested: &AtomicBool,
) -> Vec<(String, Report)> {
    let names: Vec<String> = arms.iter().map(|arm| arm.name.clone()).collect();
    let failed = |reason: String| -> Vec<(String, Report)> {
        names
            .iter()
            .map(|name| {
                let mut report = Report::empty();
                report.trip = Some(reason.clone());
                (name.clone(), report)
            })
            .collect()
    };
    let Some(first) = arms.first() else {
        return Vec::new();
    };
    if arms
        .iter()
        .any(|arm| (arm.config.frequency_hertz - first.config.frequency_hertz).abs() > f64::EPSILON)
    {
        return failed("The arms of one controller must share a control frequency".into());
    }
    let several = arms.len() > 1;
    let mut runners = Vec::new();
    for arm in arms.iter_mut() {
        let base_directory = Path::new(&arm.config.trace_directory);
        let trace_directory = if several {
            base_directory.join(&arm.name)
        } else {
            base_directory.to_path_buf()
        };
        match runner_for(
            &arm.name,
            &mut arm.plant,
            &arm.link,
            &arm.config,
            &trace_directory,
        ) {
            Ok(runner) => runners.push(runner),
            Err(report) => {
                return failed(
                    report
                        .trip
                        .unwrap_or_else(|| format!("Arm {} did not start", arm.name)),
                );
            }
        }
    }
    names
        .into_iter()
        .zip(run_runners(runners, run_for, shutdown_requested))
        .collect()
}

fn refuse_mixed_pacing<P: Plant>(runners: Vec<Runner<'_, P>>) -> Vec<Report> {
    runners
        .into_iter()
        .map(|mut runner| {
            runner.report.trip = Some(
                "Arms driven by a simulator and arms driven otherwise cannot share a controller"
                    .into(),
            );
            runner.report.trace = Some(runner.trace.finish());
            runner.report
        })
        .collect()
}

// CONSTRAINT: Apply what the arms' readers have asked since the last cycle, answering each.
// Missing requests wait for the next cycle. The batch yields at its time budget
// or 64 requests per arm, whichever comes first. Round-robin admission
// resumes at the next arm so one busy inbox cannot starve another. An individual
// request is not preemptible; its work must still fit the control budget.
fn serve_requests<P: Plant>(
    runners: &mut [Runner<'_, P>],
    budget: Duration,
    next_request_arm: &mut usize,
) {
    let began = Instant::now();
    let mut consecutive_empty_inboxes = 0;
    for _ in 0..MAXIMUM_REQUESTS_PER_CYCLE {
        for _ in 0..runners.len() {
            let index = *next_request_arm;
            *next_request_arm = (index + 1) % runners.len();
            let Some(request) = runners[index].inbox.next() else {
                consecutive_empty_inboxes += 1;
                if consecutive_empty_inboxes == runners.len() {
                    return;
                }
                continue;
            };
            consecutive_empty_inboxes = 0;
            match request {
                ArmRequest::Single(request) => request.apply(runners[index].state.arm()),
                ArmRequest::Group(request) => {
                    let mut cores: BTreeMap<&str, &mut ArmCore> = runners
                        .iter_mut()
                        .map(|runner| {
                            let core = runner.state.arm();
                            if let Some(collision) = &mut core.group_collision {
                                collision.gripper_angle = runner.feedback.gripper_angle;
                            }
                            (runner.name.as_str(), core)
                        })
                        .collect();
                    request.apply(&mut cores);
                }
            }
            if began.elapsed() >= budget {
                return;
            }
        }
    }
}

fn shutdown_every_arm<P: Plant>(runners: &mut [Runner<'_, P>]) {
    for runner in runners {
        let _ = runner.state.arm().submit(Command::Shutdown, Now::current());
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "Keep the shared timer and the two cycle halves together."
)]
fn run_runners<P: Plant>(
    mut runners: Vec<Runner<'_, P>>,
    run_for: Option<Duration>,
    shutdown_requested: &AtomicBool,
) -> Vec<Report> {
    let Some(first) = runners.first() else {
        return Vec::new();
    };
    let nominal_cycle = first.config.control_timing.period;
    let deadline = runners
        .iter()
        .map(|runner| runner.config.control_timing.deadline)
        .min()
        .unwrap_or(first.config.control_timing.deadline);
    let started = Instant::now();
    // CONSTRAINT: Simulators set the pace themselves: with them the loop does not sleep, each
    // cycle lasts the nominal period, and wall time cannot miss a deadline.
    let plant_paced = runners
        .iter()
        .all(|runner| runner.pacing().is_set_by_replies());
    if !plant_paced
        && runners
            .iter()
            .any(|runner| runner.pacing().is_set_by_replies())
    {
        return refuse_mixed_pacing(runners);
    }
    let mut timer = CycleTimer::start(nominal_cycle, deadline);
    let mut groups: Vec<ActiveGroup> = Vec::new();
    let mut next_request_arm = 0;
    let mut ended = vec![false; runners.len()];
    // WHY: Kept across cycles so a cycle allocates nothing for them.
    let mut begun: Vec<Option<PendingCycle>> = Vec::with_capacity(runners.len());
    let mut group_steps = Vec::with_capacity(runners.len());
    let mut group_writes = Vec::with_capacity(runners.len());
    let mut group_feedback =
        Vec::with_capacity(runners.iter().map(|runner| runner.config.joint_count).sum());
    if !plant_paced {
        crate::cycle::request_realtime_scheduling();
    }
    // WHY: A signal runs the same shutdown as the Shutdown command (return to rest,
    // release), given this long before the loop ends anyway.
    let mut signal_deadline: Option<Instant> = None;
    while run_for.is_none_or(|duration| started.elapsed() < duration)
        && signal_deadline.is_none_or(|limit| Instant::now() < limit)
        && ended.contains(&false)
    {
        let requests_began = Instant::now();
        // WHY: Reserve the bulk of the cycle for feedback, safety checks and writes.
        serve_requests(&mut runners, nominal_cycle / 10, &mut next_request_arm);
        let requests_seconds = requests_began.elapsed().as_secs_f64();
        if signal_deadline.is_none() && shutdown_requested.load(Ordering::Relaxed) {
            signal_deadline = Some(Instant::now() + SHUTDOWN_TIMEOUT);
            shutdown_every_arm(&mut runners);
        }
        let timer_wait_began = Instant::now();
        let (cycle, deadline_missed) = if plant_paced {
            if runners
                .iter()
                .all(|runner| runner.pacing() == Pacing::RepliesAtClockSpeed)
            {
                // CONSTRAINT: Simulated time follows the clock; a simulator slower than that
                // simply makes the cycles late, which no deadline judges.
                let _ = timer.tick();
            }
            (nominal_cycle, None)
        } else {
            match timer.tick() {
                Ok(cycle) => (cycle, None),
                Err(missed) => (
                    missed.cycle,
                    Some(deadline_missed_message(missed.cycle, host_load_averages())),
                ),
            }
        };
        let timer_wait_seconds = timer_wait_began.elapsed().as_secs_f64();
        for runner in &mut runners {
            runner.timings = crate::trace::CycleTimings {
                requests_seconds,
                timer_wait_seconds,
                actual_cycle_seconds: cycle.as_secs_f64(),
                ..Default::default()
            };
        }
        let mut restart_timer = false;
        begun.clear();
        for (index, runner) in runners.iter_mut().enumerate() {
            if ended[index] {
                // CONSTRAINT: A simulator shared with arms still running waits for this
                // arm's step too; an arm that is done keeps time moving.
                let _ = runner.advance_plant(nominal_cycle.as_secs_f64());
                begun.push(None);
                continue;
            }
            match runner.begin_cycle(cycle, deadline_missed.clone()) {
                Begun::Done(flow, restart) => {
                    restart_timer |= restart;
                    ended[index] = matches!(flow, Flow::End);
                    begun.push(None);
                }
                Begun::Pending(pending) => begun.push(Some(pending)),
            }
        }
        form_groups(&mut runners, &mut groups);
        step_groups(
            &mut runners,
            &mut groups,
            &begun,
            &mut group_steps,
            &mut group_feedback,
        );
        group_writes.clear();
        group_writes.resize_with(runners.len(), || None);
        for (index, pending) in begun.drain(..).enumerate() {
            if let Some(pending) = pending {
                let group_reference = group_steps[index].as_ref().map(|result| {
                    result
                        .as_ref()
                        .map(|index| groups[index.group].member_reference(index.member))
                });
                let finished = runners[index].finish_cycle(pending, group_reference);
                restart_timer |= finished.restart_timer;
                ended[index] = matches!(finished.flow, Flow::End);
                group_writes[index] = finished.group_write;
            }
        }
        confirm_group_writes(&mut runners, &mut groups, &mut group_writes);
        settle_groups(&mut runners, &mut groups);
        if restart_timer {
            timer.restart();
        }
    }
    runners
        .into_iter()
        .map(|mut runner| {
            if runner.startup_hold.is_some() {
                runner.fail_enable(
                    "Controller stopped before startup hold was confirmed",
                    Now::current(),
                );
            }
            runner.report.trace = Some(runner.trace.finish());
            runner.report
        })
        .collect()
}

#[cfg(test)]
mod deadline_message_tests {
    use super::*;

    #[test]
    fn a_missed_deadline_names_the_host_load_when_known() {
        let message = deadline_missed_message(Duration::from_millis(221), Some([4.8, 4.4, 4.5]));
        assert_eq!(
            message,
            "Loop deadline missed: 221ms (host load averages 1/5/15 min: 4.80 4.40 4.50)"
        );
        assert_eq!(
            deadline_missed_message(Duration::from_millis(221), None),
            "Loop deadline missed: 221ms"
        );
        assert!(host_load_averages().is_some());
    }
}

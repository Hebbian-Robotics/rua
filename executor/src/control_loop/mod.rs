//! The loop: each cycle reads the plant, checks the feedback, carries out the
//! arm state machine's next guarded setpoint, commands the
//! plant and publishes the state. A failed check is a trip: the arm holds where
//! it is and the command that was running ends tripped. The state machine
//! (`arm::ArmCore`) decides what is admitted; this decides what the servos do.

mod activity;
mod group;
mod guards;
mod learned;
mod lifecycle;
mod runner;
mod tool_pose;
mod trajectory;

#[cfg(test)]
mod lifecycle_tests;
#[cfg(test)]
mod tests;

use activity::start;
use group::{ActiveGroup, form_groups, settle_groups, step_groups};
pub(crate) use guards::{
    build_guard, build_self_collision, clamp_setpoint, guard_command, load_shared_robot,
};
use runner::{Begun, Flow, PendingCycle, Runner};

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use hdrhistogram::Histogram;

use crate::arm::{ArmCore, Now};
use crate::config::ExecutorConfig;
use crate::control_link::Link;
use crate::cycle::CycleTimer;
use crate::handoff::{ArmInbox, MAXIMUM_REQUESTS_PER_CYCLE, apply_request};
use crate::plant::{Feedback, Pacing, Plant};
use crate::trace::{TraceSummary, TraceWriter};
use hebbian_contracts::Command;

/// Cycle times are recorded in microseconds up to a minute, to three
/// significant digits: a fixed few kilobytes however long the loop runs.
pub(super) const LONGEST_RECORDED_CYCLE_MICROSECONDS: u64 = 60_000_000;
const CYCLE_TIME_SIGNIFICANT_DIGITS: u8 = 3;
/// After SIGINT or SIGTERM, how long the arms get to return to rest and
/// release before the loop ends anyway.
const SIGNAL_SHUTDOWN_SECONDS: f64 = 20.0;

pub struct Report {
    pub cycle_microseconds: Histogram<u64>,
    /// The first trip or plant failure, if any. A trip leaves the loop running;
    /// a plant failure ends the arm's run.
    pub trip: Option<String>,
    /// Cycles longer than two periods.
    pub overruns: u64,
    /// Where the cycle trace went and whether it kept up.
    pub trace: Option<TraceSummary>,
}

impl Report {
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

/// The host's 1, 5 and 15 minute load averages, when the platform reports them.
fn host_load_averages() -> Option<[f64; 3]> {
    let mut averages = [0.0_f64; 3];
    // SAFETY: getloadavg writes at most 3 doubles into the buffer it is given.
    let written = unsafe { libc::getloadavg(averages.as_mut_ptr(), 3) };
    (written == 3).then_some(averages)
}

/// What the operator sees when a cycle is late; the load says whether the
/// host, not the arm, was busy.
fn deadline_missed_message(cycle: Duration, load: Option<[f64; 3]>) -> String {
    match load {
        Some([one, five, fifteen]) => format!(
            "Loop deadline missed: {cycle:?} (host load averages 1/5/15 min: {one:.2} {five:.2} {fifteen:.2})"
        ),
        None => format!("Loop deadline missed: {cycle:?}"),
    }
}

/// Run until `run_for` elapses, or until `shutdown_requested` is set; without a
/// `run_for`, only the latter ends it. The arm starts disabled and a client
/// enables it; without a link, the arm is enabled at once and follows a slow
/// sine on every joint, for timing runs. A shutdown command (return to rest,
/// release) ends the run too, once it has finished.
pub fn run(
    plant: &mut impl Plant,
    link: Option<&Link>,
    config: &ExecutorConfig,
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

/// One arm of a station: its name, limits, plant and the socket its clients use.
pub struct ArmSetup<P: Plant> {
    pub name: String,
    pub config: ExecutorConfig,
    pub plant: P,
    pub link: Option<Link>,
}

/// Run every arm in one loop, in lockstep: each cycle reads all the arms, steps
/// the grouped motions' shared trajectories once from those readings, then
/// commands all the arms. The arms share one cycle timer, so they must share a
/// frequency. With more than one arm, each writes its cycle trace to a
/// subdirectory of its config's trace directory named for the arm. Ends as
/// [`run`] does, once every arm has finished its shutdown.
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
        return failed("The arms of one executor must share a control frequency".into());
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
            arm.link.as_ref(),
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

/// An arm's runner, or the report that says why it cannot start. Nothing runs
/// without its trace.
fn runner_for<'a, P: Plant>(
    name: &str,
    plant: &'a mut P,
    link: Option<&'a Link>,
    config: &'a ExecutorConfig,
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
    let core = link.map_or_else(
        || ArmCore::new(config.arm_limits(), |_| {}),
        |link| ArmCore::new(config.arm_limits(), link.log_publisher()),
    );
    let (inbox, samples) = link
        .and_then(Link::take_loop_end)
        .map_or((None, None), |end| (Some(end.inbox), Some(end.samples)));
    let state = match start(plant, config, core) {
        Ok(state) => state,
        Err(reason) => {
            report.trip = Some(format!("{reason:#}"));
            return Err(Box::new(report));
        }
    };
    Ok(Runner {
        name: name.to_owned(),
        plant,
        link,
        inbox,
        samples,
        feedback: Feedback::default(),
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
        recovery: runner::BusRecovery::Disabled,
        sine_stream: None,
    })
}

/// Arms whose plants set the pace and arms whose plants do not cannot run in one
/// loop: every arm ends with that as its trip.
fn refuse_mixed_pacing<P: Plant>(runners: Vec<Runner<'_, P>>) -> Vec<Report> {
    runners
        .into_iter()
        .map(|mut runner| {
            runner.report.trip = Some(
                "Arms driven by a simulator and arms driven otherwise cannot share an executor"
                    .into(),
            );
            runner.report.trace = Some(runner.trace.finish());
            runner.report
        })
        .collect()
}

/// Apply what the arms' readers have asked since the last cycle, answering each.
/// Missing requests wait for the next cycle. The batch yields after one control
/// period or 64 requests per arm, whichever comes first. Round-robin admission
/// resumes at the next arm so one busy inbox cannot starve another. An individual
/// request is not preemptible; its work must still fit the control budget.
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
            let Some(request) = runners[index].inbox.as_ref().and_then(ArmInbox::next) else {
                consecutive_empty_inboxes += 1;
                if consecutive_empty_inboxes == runners.len() {
                    return;
                }
                continue;
            };
            consecutive_empty_inboxes = 0;
            let own = runners[index].name.clone();
            let mut cores: BTreeMap<&str, &mut ArmCore> = runners
                .iter_mut()
                .map(|runner| (runner.name.as_str(), runner.state.arm()))
                .collect();
            apply_request(request, &own, &mut cores);
            if began.elapsed() >= budget {
                return;
            }
        }
    }
}

/// Ask every arm to shut down, as the Shutdown command does.
fn shutdown_every_arm<P: Plant>(runners: &mut [Runner<'_, P>]) {
    for runner in runners {
        let _ = runner.state.arm().submit(Command::Shutdown, Now::current());
    }
}

/// The loop over every arm's runner. Group handling sits between each cycle's
/// two halves.
#[allow(clippy::too_many_lines)] // Keep the shared timer and the two cycle halves together.
fn run_runners<P: Plant>(
    mut runners: Vec<Runner<'_, P>>,
    run_for: Option<Duration>,
    shutdown_requested: &AtomicBool,
) -> Vec<Report> {
    let Some(first) = runners.first() else {
        return Vec::new();
    };
    let frequency = first.config.frequency_hertz;
    let deadline = runners
        .iter()
        .map(|runner| runner.config.loop_deadline_seconds)
        .fold(f64::INFINITY, f64::min);
    for runner in &mut runners {
        if runner.link.is_none() {
            let _ = runner.state.arm().submit(Command::Enable, Now::current());
        }
    }
    let started = Instant::now();
    // Simulators set the pace themselves: with them the loop does not sleep, each
    // cycle lasts the nominal period, and wall time cannot miss a deadline.
    let plant_paced = runners
        .iter()
        .all(|runner| runner.plant.pacing().is_set_by_replies());
    if !plant_paced
        && runners
            .iter()
            .any(|runner| runner.plant.pacing().is_set_by_replies())
    {
        return refuse_mixed_pacing(runners);
    }
    let nominal_cycle = Duration::from_secs_f64(1.0 / frequency);
    let mut timer = CycleTimer::start(frequency, Duration::from_secs_f64(deadline));
    let mut groups: Vec<ActiveGroup> = Vec::new();
    let mut next_request_arm = 0;
    let mut ended = vec![false; runners.len()];
    // Kept across cycles so a cycle allocates nothing for them.
    let mut begun: Vec<Option<PendingCycle>> = Vec::with_capacity(runners.len());
    let mut group_steps = Vec::with_capacity(runners.len());
    if !plant_paced {
        crate::cycle::request_realtime_scheduling();
    }
    // A signal runs the same shutdown as the Shutdown command (return to rest,
    // release), given this long before the loop ends anyway.
    let mut signal_deadline: Option<Instant> = None;
    while run_for.is_none_or(|duration| started.elapsed() < duration)
        && signal_deadline.is_none_or(|limit| Instant::now() < limit)
        && ended.contains(&false)
    {
        let requests_began = Instant::now();
        serve_requests(&mut runners, nominal_cycle, &mut next_request_arm);
        let requests_seconds = requests_began.elapsed().as_secs_f64();
        if signal_deadline.is_none() && shutdown_requested.load(Ordering::Relaxed) {
            signal_deadline =
                Some(Instant::now() + Duration::from_secs_f64(SIGNAL_SHUTDOWN_SECONDS));
            shutdown_every_arm(&mut runners);
        }
        let timer_wait_began = Instant::now();
        let (cycle, deadline_missed) = if plant_paced {
            if runners
                .iter()
                .all(|runner| runner.plant.pacing() == Pacing::RepliesAtClockSpeed)
            {
                // Simulated time follows the clock; a simulator slower than that
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
                // A simulator shared with arms still running waits for this
                // arm's step too; an arm that is done keeps time moving.
                let _ = runner.plant.advance(nominal_cycle.as_secs_f64());
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
        form_groups(&runners, &mut groups);
        step_groups(&runners, &mut groups, &begun, &mut group_steps);
        for (index, pending) in begun.drain(..).enumerate() {
            if let Some(pending) = pending {
                let (flow, restart) =
                    runners[index].finish_cycle(pending, group_steps[index].as_ref());
                restart_timer |= restart;
                ended[index] = matches!(flow, Flow::End);
            }
        }
        settle_groups(&mut runners, &mut groups);
        if restart_timer {
            timer.restart();
        }
    }
    runners
        .into_iter()
        .map(|mut runner| {
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

//! Tests of enabling, bus failures, shutdown and trip clearing against a plant
//! that follows a script.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use super::runner::{Begun, BusRecovery, Flow, Runner};
use super::runner_for;
use crate::arm::Now;
use crate::config::{EnableChecks, ExecutorConfig};
use crate::plant::{Feedback, Plant, RecoveryObservation};
use hebbian_contracts::{ArmMode, Command};

/// How often the loop called the plant, readable after the runner has the plant.
#[derive(Default, Clone)]
struct Calls {
    enable: Arc<AtomicU32>,
    release: Arc<AtomicU32>,
    guard_start: Arc<AtomicU32>,
    recovery_hold: Arc<AtomicU32>,
}

fn count(counter: &Arc<AtomicU32>) -> u32 {
    counter.load(Ordering::SeqCst)
}

/// A plant that reads what it is given, then keeps repeating the last reading.
struct ScriptedPlant {
    readings: VecDeque<Feedback>,
    last: Feedback,
    heartbeat_fails: bool,
    /// The next this many reads go unanswered, as a bus that was unplugged.
    unanswered_reads: u32,
    recovery_unanswered_reads: u32,
    recovery_torque: Vec<bool>,
    calls: Calls,
}

impl ScriptedPlant {
    fn new(readings: Vec<Feedback>, calls: &Calls) -> Self {
        Self {
            last: readings[0].clone(),
            readings: readings.into(),
            heartbeat_fails: false,
            unanswered_reads: 0,
            recovery_unanswered_reads: 0,
            recovery_torque: vec![true, true],
            calls: calls.clone(),
        }
    }
}

impl Plant for ScriptedPlant {
    fn read_feedback_into(&mut self, feedback: &mut Feedback) -> anyhow::Result<()> {
        if self.unanswered_reads > 0 {
            self.unanswered_reads -= 1;
            anyhow::bail!("no answer");
        }
        if let Some(next) = self.readings.pop_front() {
            self.last = next;
        }
        feedback.positions.clone_from(&self.last.positions);
        feedback.velocities.clone_from(&self.last.velocities);
        feedback.efforts.clone_from(&self.last.efforts);
        feedback
            .temperatures_celsius
            .clone_from(&self.last.temperatures_celsius);
        feedback
            .voltages_volts
            .clone_from(&self.last.voltages_volts);
        feedback
            .environment_confirmations
            .clone_from(&self.last.environment_confirmations);
        feedback.gripper_angle = self.last.gripper_angle;
        feedback.fault.clone_from(&self.last.fault);
        Ok(())
    }

    fn command(&mut self, _positions: &[f64], _seconds: f64) -> anyhow::Result<()> {
        Ok(())
    }

    fn command_gripper(&mut self, _angle: f64) -> anyhow::Result<()> {
        Ok(())
    }

    fn preflight(&mut self) -> anyhow::Result<()> {
        Ok(())
    }

    fn enable(&mut self) -> anyhow::Result<()> {
        self.calls.enable.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn release(&mut self) -> anyhow::Result<()> {
        self.calls.release.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn start_guard(&mut self) -> anyhow::Result<()> {
        self.calls.guard_start.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn heartbeat(&mut self) -> anyhow::Result<()> {
        anyhow::ensure!(!self.heartbeat_fails, "the guard is gone");
        Ok(())
    }

    fn observe_recovery(&mut self) -> anyhow::Result<RecoveryObservation> {
        if self.recovery_unanswered_reads > 0 {
            self.recovery_unanswered_reads -= 1;
            return Err(crate::plant::CommunicationFailure("no answer".into()).into());
        }
        Ok(RecoveryObservation {
            feedback: self.read_feedback()?,
            motor_torque: self
                .recovery_torque
                .iter()
                .enumerate()
                .map(|(index, enabled)| (u8::try_from(index + 1).unwrap(), *enabled))
                .collect(),
            health_issue: None,
        })
    }

    fn hold_recovery(&mut self) -> anyhow::Result<RecoveryObservation> {
        self.calls.recovery_hold.fetch_add(1, Ordering::SeqCst);
        let observation = self.observe_recovery()?;
        anyhow::ensure!(
            observation.motor_torque.iter().all(|(_, enabled)| *enabled),
            "A motor has no torque"
        );
        Ok(observation)
    }

    fn enable_recovery(
        &mut self,
        _drift_limit_radians: f64,
    ) -> anyhow::Result<RecoveryObservation> {
        self.calls.enable.fetch_add(1, Ordering::SeqCst);
        self.recovery_torque.fill(true);
        self.observe_recovery()
    }
}

fn at(positions: [f64; 2]) -> Feedback {
    Feedback {
        positions: positions.to_vec(),
        velocities: vec![0.0; 2],
        efforts: vec![0.0; 2],
        temperatures_celsius: Vec::new(),
        voltages_volts: Vec::new(),
        environment_confirmations: Vec::new(),
        gripper_angle: None,
        fault: None,
    }
}

fn config(test_name: &str) -> ExecutorConfig {
    let mut config = ExecutorConfig::parse(crate::config::TEST_CONFIG)
        .unwrap()
        .into_input();
    config.enable_checks = Some(EnableChecks {
        preflight_drift_limit_radians: 0.02,
        enable_drift_limit_radians: 0.05,
    });
    config.trace_directory = std::env::temp_dir()
        .join(format!("lifecycle-{test_name}-{}", std::process::id()))
        .to_string_lossy()
        .into_owned();
    ExecutorConfig::try_from(config).unwrap()
}

fn runner<'a>(
    plant: &'a mut ScriptedPlant,
    config: &'a ExecutorConfig,
) -> Runner<'a, ScriptedPlant> {
    let Ok(runner) = runner_for(
        "arm",
        plant,
        None,
        config,
        std::path::Path::new(&config.trace_directory),
    ) else {
        panic!("the runner starts");
    };
    runner
}

/// The runner's first reading is taken by its start, so a script's first entry is
/// that one.
fn enabling_error(readings: Vec<Feedback>, test_name: &str) -> (String, Calls) {
    let calls = Calls::default();
    let config = config(test_name);
    let mut plant = ScriptedPlant::new(readings, &calls);
    let mut runner = runner(&mut plant, &config);
    let Err(refusal) = runner.enable_checked() else {
        panic!("the enable is refused");
    };
    let refusal = refusal.to_string();
    (refusal, calls)
}

#[test]
fn a_joint_outside_its_limits_refuses_the_enable_before_any_torque() {
    let (refusal, calls) = enabling_error(vec![at([0.0, 0.0]), at([2.5, 0.0])], "limits");
    assert!(refusal.contains("outside its limits"), "{refusal}");
    assert_eq!(count(&calls.enable), 0);
    assert_eq!(count(&calls.guard_start), 0);
}

#[test]
fn an_arm_that_moves_during_the_preflight_is_refused_with_the_guard_started_but_no_torque() {
    let (refusal, calls) = enabling_error(
        vec![at([0.0, 0.0]), at([0.0, 0.0]), at([0.1, 0.0])],
        "preflight-drift",
    );
    assert_eq!(refusal, "Moved during preflight");
    assert_eq!(count(&calls.guard_start), 1);
    assert_eq!(count(&calls.enable), 0);
}

#[test]
fn an_arm_that_moves_while_torque_goes_on_is_refused() {
    let (refusal, calls) = enabling_error(
        vec![
            at([0.0, 0.0]),
            at([0.0, 0.0]),
            at([0.0, 0.0]),
            at([0.2, 0.0]),
        ],
        "enable-drift",
    );
    assert_eq!(refusal, "Moved during enable");
    assert_eq!(count(&calls.enable), 1);
}

#[test]
fn a_servo_fault_reported_with_the_enable_reading_refuses_it() {
    let mut faulty = at([0.0, 0.0]);
    faulty.fault = Some("1: voltage".into());
    let (refusal, _) = enabling_error(
        vec![at([0.0, 0.0]), at([0.0, 0.0]), at([0.0, 0.0]), faulty],
        "enable-fault",
    );
    assert!(refusal.contains("Enable not confirmed"), "{refusal}");
}

#[test]
fn a_refused_enable_releases_whatever_the_attempt_left_on_and_fails_the_command() {
    let calls = Calls::default();
    let config = config("refused-enable");
    let mut plant = ScriptedPlant::new(vec![at([0.0, 0.0]), at([2.5, 0.0])], &calls);
    let mut runner = runner(&mut plant, &config);
    let now = Now::current();
    runner.state.arm().submit(Command::Enable, now).unwrap();
    runner.state.arm().next_job();
    runner.enable(now);
    assert_eq!(count(&calls.release), 1);
    assert_eq!(runner.state.arm().mode(), ArmMode::Disabled);
    assert!(!runner.state.powered);
}

#[test]
fn a_clean_enable_turns_torque_on_once_and_powers_the_loop_up() {
    let calls = Calls::default();
    let config = config("clean-enable");
    let mut plant = ScriptedPlant::new(vec![at([0.0, 0.0])], &calls);
    let mut runner = runner(&mut plant, &config);
    let now = Now::current();
    runner.state.arm().submit(Command::Enable, now).unwrap();
    runner.state.arm().next_job();
    runner.enable(now);
    assert_eq!(count(&calls.enable), 1);
    assert!(runner.state.powered);
    assert_eq!(runner.state.arm().mode(), ArmMode::Holding);
}

fn powered_runner<'a>(
    plant: &'a mut ScriptedPlant,
    config: &'a ExecutorConfig,
) -> Runner<'a, ScriptedPlant> {
    let mut runner = runner(plant, config);
    runner.state.power_up(&[0.0, 0.0], None);
    runner.state.arm().enabled(&[0.0, 0.0], Now::current());
    runner
}

#[test]
fn a_bus_failure_while_enabled_is_waited_out_with_the_guard_still_heard() {
    let calls = Calls::default();
    let config = config("patience");
    let mut plant = ScriptedPlant::new(vec![at([0.0, 0.0])], &calls);
    let mut runner = powered_runner(&mut plant, &config);
    runner.bus_failed(&anyhow::anyhow!("no answer"), Now::current());
    assert!(runner.state.powered);
    assert!(runner.report.trip.is_none());
    assert_eq!(count(&calls.release), 0);
}

#[test]
fn a_lost_bus_recovers_a_verified_hold_without_releasing_torque() {
    let calls = Calls::default();
    let config = config("bus-recovery");
    let mut plant = ScriptedPlant::new(vec![at([0.0, 0.0])], &calls);
    let cycles_in_grace = 49;
    let failed_reconnections = 3;
    let mut runner = powered_runner(&mut plant, &config);
    runner.plant.unanswered_reads = cycles_in_grace + 1 + failed_reconnections;
    let cycle = |runner: &mut Runner<'_, ScriptedPlant>| {
        if let Begun::Done(flow, _) = runner.begin_cycle(Duration::from_millis(10), None) {
            assert!(matches!(flow, Flow::Continue), "the run goes on");
        }
    };
    for _ in 0..cycles_in_grace {
        cycle(&mut runner);
        assert!(runner.state.powered, "still holding within the grace");
    }
    cycle(&mut runner);
    assert!(!runner.state.powered);
    assert_eq!(runner.state.arm().mode(), ArmMode::Disconnected);
    assert_eq!(count(&calls.release), 0);
    assert!(
        runner
            .report
            .trip
            .as_deref()
            .is_some_and(|trip| trip.contains("no answer"))
    );
    let refusal = runner
        .state
        .arm()
        .submit(Command::Enable, Now::current())
        .unwrap_err();
    assert!(
        refusal.detail.contains("disconnected"),
        "{}",
        refusal.detail
    );

    for _ in 0..failed_reconnections {
        cycle(&mut runner);
        assert_eq!(runner.state.arm().mode(), ArmMode::Disconnected);
    }
    cycle(&mut runner);
    assert_eq!(runner.state.arm().mode(), ArmMode::Disconnected);
    std::thread::sleep(Duration::from_millis(120));
    cycle(&mut runner);
    assert_eq!(count(&calls.release), 0);
    assert_eq!(count(&calls.enable), 0);
    assert!(runner.state.powered);
    assert_eq!(runner.state.arm().mode(), ArmMode::Holding);
}

#[test]
fn another_communication_failure_during_recovery_retries_the_full_hold_check() {
    let calls = Calls::default();
    let config = config("recovery-disconnect");
    let mut plant = ScriptedPlant::new(vec![at([0.0, 0.0])], &calls);
    let mut runner = powered_runner(&mut plant, &config);
    for _ in 0..50 {
        runner.bus_failed(&anyhow::anyhow!("no answer"), Now::current());
    }
    runner.plant.recovery_unanswered_reads = 1;
    runner.begin_cycle(Duration::from_millis(10), None);
    assert!(matches!(runner.recovery, BusRecovery::Reconnecting));
    assert_eq!(runner.state.arm().mode(), ArmMode::Disconnected);
    runner.begin_cycle(Duration::from_millis(10), None);
    assert!(matches!(runner.recovery, BusRecovery::VerifyingHold { .. }));
    runner.plant.recovery_unanswered_reads = 1;
    runner.begin_cycle(Duration::from_millis(10), None);
    assert!(matches!(runner.recovery, BusRecovery::Reconnecting));
    runner.begin_cycle(Duration::from_millis(10), None);
    assert!(matches!(runner.recovery, BusRecovery::VerifyingHold { .. }));
    std::thread::sleep(Duration::from_millis(120));
    runner.begin_cycle(Duration::from_millis(10), None);
    assert_eq!(runner.state.arm().mode(), ArmMode::Holding);
    assert_eq!(count(&calls.release), 0);
    assert_eq!(count(&calls.enable), 0);
}

#[test]
fn an_out_of_limits_reconnection_never_commands_a_recovery_hold() {
    let calls = Calls::default();
    let config = config("recovery-outside-limits");
    let mut plant = ScriptedPlant::new(vec![at([0.0, 0.0])], &calls);
    let mut runner = powered_runner(&mut plant, &config);
    for _ in 0..50 {
        runner.bus_failed(&anyhow::anyhow!("no answer"), Now::current());
    }
    runner.plant.readings.clear();
    runner.plant.last.positions[0] =
        config.upper_limits[0] + config.joint_limit_recovery_tolerance_radians + 0.1;
    runner.begin_cycle(Duration::from_millis(10), None);
    assert!(matches!(runner.recovery, BusRecovery::AwaitingApproval(_)));
    assert_eq!(count(&calls.recovery_hold), 0);
    assert_eq!(count(&calls.release), 0);
    assert_eq!(count(&calls.enable), 0);
}

#[test]
fn a_reconnected_bus_with_a_motor_unpowered_does_not_resume_or_release() {
    let calls = Calls::default();
    let config = config("mixed-torque-recovery");
    let mut plant = ScriptedPlant::new(vec![at([0.0, 0.0])], &calls);
    let mut runner = powered_runner(&mut plant, &config);
    runner.plant.recovery_torque[1] = false;
    for _ in 0..50 {
        runner.bus_failed(&anyhow::anyhow!("no answer"), Now::current());
    }
    for _ in 0..3 {
        runner.begin_cycle(Duration::from_millis(10), None);
    }
    assert_eq!(runner.state.arm().mode(), ArmMode::Disconnected);
    assert!(!runner.state.powered);
    assert!(matches!(
        &runner.recovery,
        BusRecovery::AwaitingApproval(issue) if issue.motors_without_torque == [2]
    ));
    assert_eq!(count(&calls.release), 0);
    assert_eq!(count(&calls.enable), 0);

    let record = runner
        .state
        .arm()
        .submit(Command::ApproveRecovery, Now::current())
        .unwrap();
    runner.begin_cycle(Duration::from_millis(10), None);
    assert_eq!(count(&calls.enable), 1);
    assert_eq!(runner.state.arm().mode(), ArmMode::Disconnected);
    std::thread::sleep(Duration::from_millis(120));
    runner.begin_cycle(Duration::from_millis(10), None);
    assert_eq!(runner.state.arm().mode(), ArmMode::Holding);
    assert_eq!(
        runner
            .state
            .arm()
            .record(&record.command_id)
            .unwrap()
            .outcome(),
        Some(hebbian_contracts::CommandOutcome::Completed)
    );
    assert_eq!(count(&calls.release), 0);
}

#[test]
fn stopping_an_approved_recovery_keeps_the_arm_paused() {
    let calls = Calls::default();
    let config = config("stop-recovery");
    let mut plant = ScriptedPlant::new(vec![at([0.0, 0.0])], &calls);
    let mut runner = powered_runner(&mut plant, &config);
    runner.plant.recovery_torque[1] = false;
    for _ in 0..50 {
        runner.bus_failed(&anyhow::anyhow!("no answer"), Now::current());
    }
    runner.begin_cycle(Duration::from_millis(10), None);
    let approval = runner
        .state
        .arm()
        .submit(Command::ApproveRecovery, Now::current())
        .unwrap();
    runner.begin_cycle(Duration::from_millis(10), None);
    runner
        .state
        .arm()
        .submit(Command::Stop, Now::current())
        .unwrap();
    runner.begin_cycle(Duration::from_millis(10), None);
    assert_eq!(runner.state.arm().mode(), ArmMode::Disconnected);
    assert_eq!(
        runner
            .state
            .arm()
            .record(&approval.command_id)
            .unwrap()
            .outcome(),
        Some(hebbian_contracts::CommandOutcome::Failed)
    );
    assert!(matches!(runner.recovery, BusRecovery::AwaitingApproval(_)));
    assert_eq!(count(&calls.release), 0);
}

#[test]
fn a_heartbeat_that_fails_during_patience_counts_as_a_plant_failure() {
    let calls = Calls::default();
    let config = config("heartbeat");
    let mut plant = ScriptedPlant::new(vec![at([0.0, 0.0])], &calls);
    plant.heartbeat_fails = true;
    let mut runner = powered_runner(&mut plant, &config);
    runner.bus_failed(&anyhow::anyhow!("no answer"), Now::current());
    assert!(!runner.state.powered);
    assert!(
        runner
            .report
            .trip
            .as_deref()
            .is_some_and(|trip| trip.contains("the guard is gone"))
    );
    assert_eq!(count(&calls.release), 1);
}

#[test]
fn a_shutdown_at_the_rest_pose_releases_without_moving() {
    let calls = Calls::default();
    let config = config("shutdown-at-rest");
    let mut plant = ScriptedPlant::new(vec![at([0.0, 0.0])], &calls);
    let mut runner = powered_runner(&mut plant, &config);
    let now = Now::current();
    runner.state.arm().submit(Command::Shutdown, now).unwrap();
    assert!(runner.shutdown_step(now));
    assert_eq!(count(&calls.release), 1);
    assert!(!runner.state.powered);
}

#[test]
fn a_shutdown_away_from_rest_returns_once_and_then_releases() {
    let calls = Calls::default();
    let config = config("shutdown-away");
    let mut plant = ScriptedPlant::new(vec![at([0.0, 0.0])], &calls);
    let mut runner = powered_runner(&mut plant, &config);
    let now = Now::current();
    runner.state.previous_command = vec![1.0, 0.0];
    runner.state.arm().submit(Command::Shutdown, now).unwrap();
    assert!(!runner.shutdown_step(now), "the return to rest begins");
    assert!(runner.state.shutdown_return_attempted);
    assert_eq!(count(&calls.release), 0);
    assert!(runner.state.powered);
    assert!(runner.shutdown_step(now), "a second attempt releases");
    assert_eq!(count(&calls.release), 1);
}

#[test]
fn a_shutdown_not_requested_or_an_unpowered_arm_does_nothing() {
    let calls = Calls::default();
    let config = config("shutdown-idle");
    let mut plant = ScriptedPlant::new(vec![at([0.0, 0.0])], &calls);
    let mut runner = powered_runner(&mut plant, &config);
    assert!(!runner.shutdown_step(Now::current()));
    assert!(runner.state.powered);
    assert_eq!(count(&calls.release), 0);
}

#[test]
fn a_trip_clears_when_the_checks_pass_again_and_stays_while_they_fail() {
    let calls = Calls::default();
    let config = config("trip-clearing");
    let mut faulty = at([0.0, 0.0]);
    faulty.fault = Some("1: overheating".into());
    let mut plant = ScriptedPlant::new(vec![at([0.0, 0.0]), faulty, at([0.0, 0.0])], &calls);
    let mut runner = powered_runner(&mut plant, &config);
    runner.state.tripped = Some("earlier".into());
    let cycle = Duration::from_millis(10);
    runner.begin_cycle(cycle, None);
    assert!(
        runner.state.tripped.is_some(),
        "the fault still fails the check"
    );
    runner.begin_cycle(cycle, None);
    assert!(
        runner.state.tripped.is_none(),
        "healthy feedback clears the trip"
    );
}

#[test]
fn the_loop_answers_a_bounded_number_of_requests_each_cycle_and_the_rest_next_cycle() {
    let calls = Calls::default();
    let config = config("bounded-requests");
    let mut plant = ScriptedPlant::new(vec![at([0.0, 0.0])], &calls);
    let mut runner = runner(&mut plant, &config);
    let (handle, inbox) = crate::handoff::arm_channel();
    runner.inbox = Some(inbox);
    let excess = 6;
    let answers: Vec<_> = (0..crate::handoff::MAXIMUM_REQUESTS_PER_CYCLE + excess)
        .map(|_| handle.submit_unawaited(Command::Stop, None).unwrap())
        .collect();
    let answered = |answers: &[std::sync::mpsc::Receiver<_>]| {
        answers
            .iter()
            .filter(|answer| answer.try_recv().is_ok())
            .count()
    };

    super::serve_requests(
        std::slice::from_mut(&mut runner),
        Duration::from_millis(10),
        &mut 0,
    );
    assert_eq!(
        answered(&answers),
        crate::handoff::MAXIMUM_REQUESTS_PER_CYCLE
    );
    super::serve_requests(
        std::slice::from_mut(&mut runner),
        Duration::from_millis(10),
        &mut 0,
    );
    assert_eq!(answered(&answers), excess);
}

#[test]
fn a_cycle_with_nothing_asked_of_it_serves_nothing_and_does_not_wait() {
    let calls = Calls::default();
    let config = config("idle-inbox");
    let mut plant = ScriptedPlant::new(vec![at([0.0, 0.0])], &calls);
    let mut runner = runner(&mut plant, &config);
    let (_handle, inbox) = crate::handoff::arm_channel();
    runner.inbox = Some(inbox);
    super::serve_requests(
        std::slice::from_mut(&mut runner),
        Duration::from_millis(10),
        &mut 0,
    );
    assert_eq!(runner.state.arm().mode(), ArmMode::Disabled);
}

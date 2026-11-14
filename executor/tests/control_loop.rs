//! The control loop follows commands from link clients and reports back what it did.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use hebbian_robot_use_server::config::ExecutorConfig;
use hebbian_robot_use_server::control_link::Link;
use hebbian_robot_use_server::control_loop::{ArmSetup, run, run_arms};
use hebbian_robot_use_server::lease::{LEASE, Lease};
use hebbian_robot_use_server::plant::{Feedback, Plant, SimulatedPlant};
use serde_json::{Value, json};

mod support;
use hebbian_contracts::{ArmMode, CommandOutcome, Event, Sample};
use support::{LinkClient, RunningArms};

const CONFIG: &str = r#"{
  "urdf": "unused.urdf", "driven_joints": ["a", "b"], "surface_guard": null,
  "motion": {"acceleration_limit": 20.0, "jerk_limit": 200.0, "settled_error_radians": 0.02, "settle_dwell_seconds": 0.1, "settle_timeout_seconds": 2.0}, "joint_count": 2, "frequency_hertz": 100.0, "loop_deadline_seconds": 0.1,
  "lower_limits": [-2.0, -2.0], "upper_limits": [2.0, 2.0],
  "joint_effort_limits": [10.0, 10.0], "tracking_following_lag_seconds": 0.05, "tracking_static_margin_radians": 1.5,
  "max_joint_speed": 3.0, "default_joint_speed": 1.5,
  "stream_goal_timeout_seconds": 5.0,
  "joint_limit_recovery_tolerance_radians": 0.0349, "jaw_closing_axis": "x"
}"#;

/// A lease for one executor, which its arms share.
fn control() -> Arc<Lease> {
    Arc::new(Lease::new(LEASE))
}

/// The test configuration, with its cycle traces kept out of the source tree.
fn test_config() -> ExecutorConfig {
    let mut config = ExecutorConfig::parse(CONFIG).unwrap().into_input();
    config.trace_directory = std::env::temp_dir()
        .join("hebbian-robot-use-server-test-traces")
        .to_string_lossy()
        .into_owned();
    ExecutorConfig::try_from(config).unwrap()
}

fn in_mode(mode: ArmMode) -> impl FnMut(&Event) -> bool {
    move |event| matches!(event, Event::State(sample) if sample.mode == mode)
}

/// A simulated arm whose servos report a fault shortly after torque goes on.
struct FaultingPlant {
    inner: SimulatedPlant,
    reads_since_enable: Option<u32>,
}

impl Plant for FaultingPlant {
    fn read_feedback_into(&mut self, feedback: &mut Feedback) -> anyhow::Result<()> {
        self.inner.read_feedback_into(feedback)?;
        feedback.temperatures_celsius = vec![70.0, 31.0];
        if let Some(reads) = &mut self.reads_since_enable {
            *reads += 1;
            if *reads > 2 {
                feedback.fault = Some("1: ok 70C".to_owned());
            }
        }
        Ok(())
    }

    fn command(&mut self, positions: &[f64], seconds: f64) -> anyhow::Result<()> {
        self.inner.command(positions, seconds)
    }

    fn command_gripper(&mut self, angle: f64) -> anyhow::Result<()> {
        self.inner.command_gripper(angle)
    }

    fn preflight(&mut self) -> anyhow::Result<()> {
        self.inner.preflight()
    }

    fn enable(&mut self) -> anyhow::Result<()> {
        self.reads_since_enable = Some(0);
        self.inner.enable()
    }

    fn release(&mut self) -> anyhow::Result<()> {
        self.inner.release()
    }
}

#[test]
fn a_tripped_loop_trips_once_and_keeps_reporting_the_fault() {
    let config = test_config();
    let link = Link::new(&config, control()).unwrap();
    let handle = link.handle();
    let mut client = LinkClient::connect(&handle);
    let loop_thread = std::thread::spawn(move || {
        let mut plant = FaultingPlant {
            inner: SimulatedPlant::at(vec![0.0, 0.0], 0.02),
            reads_since_enable: None,
        };
        let report = run(
            &mut plant,
            Some(&link),
            &config,
            Some(Duration::from_millis(500)),
            &AtomicBool::new(false),
        );
        link.close();
        report
    });

    client.send_command(None, &json!({"kind": "enable"}));
    let report = loop_thread.join().unwrap();
    let events = client.drain_events();

    assert_eq!(report.trip.as_deref(), Some("1: ok 70C"));
    let trips = events
        .iter()
        .filter(|event| matches!(event, Event::Tripped { .. }))
        .count();
    assert_eq!(trips, 1, "a held arm is not tripped again");
    let states: Vec<&Sample> = events
        .iter()
        .filter_map(|event| match event {
            Event::State(sample) => Some(sample),
            _ => None,
        })
        .collect();
    assert!(states.len() > 10);
    // Once the fault appears it stays in every sample, tripped or not.
    let first_faulted = states
        .iter()
        .position(|state| state.fault.is_some())
        .expect("the fault shows");
    for state in &states[first_faulted..] {
        assert_eq!(state.fault.as_deref(), Some("1: ok 70C"));
        assert_eq!(state.temperatures_celsius, vec![70.0, 31.0]);
    }
}

#[test]
fn the_loop_runs_until_shutdown_is_requested() {
    let config = test_config();
    let shutdown_requested = Arc::new(AtomicBool::new(false));
    let requester = Arc::clone(&shutdown_requested);
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(200));
        requester.store(true, Ordering::Relaxed);
    });
    let mut plant = SimulatedPlant::at(vec![0.0, 0.0], 0.02);
    let report = run(&mut plant, None, &config, None, &shutdown_requested);
    assert!(report.cycle_microseconds.len() >= 10);
    assert!(report.trip.is_none());
}

#[test]
fn a_simulated_arm_recovers_from_just_outside_a_joint_limit() {
    let config = test_config();
    let link = Link::new(&config, control()).unwrap();
    let handle = link.handle();
    let shutdown_requested = Arc::new(AtomicBool::new(false));
    let loop_shutdown_requested = Arc::clone(&shutdown_requested);
    let loop_thread = std::thread::spawn(move || {
        let mut plant = SimulatedPlant::at(vec![2.02, 0.0], 0.02);
        let report = run(
            &mut plant,
            Some(&link),
            &config,
            Some(Duration::from_secs(4)),
            &loop_shutdown_requested,
        );
        link.close();
        report
    });
    let mut client = LinkClient::connect(&handle);
    client.send_command(None, &json!({"kind": "enable"}));
    client.read_until(in_mode(ArmMode::Holding));
    client.send_command(
        None,
        &json!({
            "kind": "move", "label": "recover", "joint_speed": 1.0,
            "plan": {"peak_speed": 1.0, "segments": [{
                "label": "recover", "corners": [[2.02, 0.0], [1.8, 0.0]]
            }]}
        }),
    );
    let outcome = client.read_until(|event| {
        matches!(event, Event::Record(record) if record.label == "recover" && record.outcome().is_some())
            || matches!(event, Event::Rejected { .. })
    });
    assert!(
        matches!(outcome, Event::Record(ref record) if record.outcome() == Some(CommandOutcome::Completed)),
        "{outcome:?}"
    );
    shutdown_requested.store(true, Ordering::Relaxed);
    assert!(loop_thread.join().unwrap().trip.is_none());
}

/// Two arms of one executor, each with its own link, in one loop.
struct TwoArms {
    left: LinkClient,
    right: LinkClient,
    /// Setting an arm's flag makes its servos report a fault.
    faults: [Arc<AtomicBool>; 2],
    _running_arms: RunningArms,
}

/// A simulated arm whose servos report a fault while its flag is set.
struct SwitchedFaultPlant {
    inner: SimulatedPlant,
    fault: Arc<AtomicBool>,
}

impl Plant for SwitchedFaultPlant {
    fn read_feedback_into(&mut self, feedback: &mut Feedback) -> anyhow::Result<()> {
        self.inner.read_feedback_into(feedback)?;
        if self.fault.load(Ordering::Relaxed) {
            feedback.fault = Some("overheating".to_owned());
        }
        Ok(())
    }

    fn command(&mut self, positions: &[f64], seconds: f64) -> anyhow::Result<()> {
        self.inner.command(positions, seconds)
    }

    fn command_gripper(&mut self, angle: f64) -> anyhow::Result<()> {
        self.inner.command_gripper(angle)
    }

    fn preflight(&mut self) -> anyhow::Result<()> {
        self.inner.preflight()
    }

    fn enable(&mut self) -> anyhow::Result<()> {
        self.inner.enable()
    }

    fn release(&mut self) -> anyhow::Result<()> {
        self.inner.release()
    }
}

fn two_arms() -> TwoArms {
    let faults = [
        Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicBool::new(false)),
    ];
    let control = control();
    let setups: Vec<ArmSetup<SwitchedFaultPlant>> = ["left", "right"]
        .iter()
        .zip(&faults)
        .map(|(name, fault)| {
            let config = test_config();
            let link = Link::new(&config, Arc::clone(&control)).unwrap();
            ArmSetup {
                name: (*name).to_owned(),
                config,
                plant: SwitchedFaultPlant {
                    inner: SimulatedPlant::at(vec![0.0, 0.0], 0.02),
                    fault: Arc::clone(fault),
                },
                link: Some(link),
            }
        })
        .collect();
    let handles: Vec<_> = setups
        .iter()
        .filter_map(|setup| setup.link.as_ref().map(Link::handle))
        .collect();
    let running_arms = RunningArms::start(setups);
    let mut left = LinkClient::connect(&handles[0]);
    let mut right = LinkClient::connect(&handles[1]);
    for client in [&mut left, &mut right] {
        client.send_command(None, &json!({"kind": "enable"}));
        client.read_until(in_mode(ArmMode::Holding));
    }
    TwoArms {
        left,
        right,
        faults,
        _running_arms: running_arms,
    }
}

fn move_arms(client: &LinkClient, request_id: &str, targets: [&Value; 2]) {
    let part = |arm: &str, target: &Value| {
        json!({
            "arm": arm, "joint_speed": 1.0,
            "plan": {"peak_speed": 1.0, "segments": [{
                "label": "group_target", "corners": [[0.0, 0.0], target]
            }]}
        })
    };
    client.send_request(
        Some(request_id),
        &json!({"kind": "move_arms", "label": "move_arms",
                    "moves": [part("left", targets[0]), part("right", targets[1])]}),
    );
}

fn ended_record(client: &mut LinkClient) -> hebbian_contracts::CommandRecord {
    let Event::Record(record) = client.read_until(
        |event| matches!(event, Event::Record(record) if record.label == "move_arms" && record.outcome().is_some()),
    ) else {
        unreachable!()
    };
    record
}

#[test]
fn a_grouped_move_takes_every_arm_to_its_target_together() {
    let mut arms = two_arms();
    move_arms(
        &arms.left,
        "group",
        [&json!([0.6, 0.0]), &json!([0.0, -0.3])],
    );
    let Event::Reply {
        accepted, records, ..
    } = arms
        .left
        .read_until(|event| matches!(event, Event::Reply { .. }))
    else {
        unreachable!()
    };
    assert!(accepted);
    assert_eq!(records.len(), 2);
    assert!(records[0].group_id.is_some());
    assert_eq!(records[0].group_id, records[1].group_id);

    let left = ended_record(&mut arms.left);
    let right = ended_record(&mut arms.right);
    assert_eq!(left.outcome(), Some(CommandOutcome::Completed), "{left:?}");
    assert_eq!(
        right.outcome(),
        Some(CommandOutcome::Completed),
        "{right:?}"
    );
    // One trajectory phase: the arm with the shorter way is not done early.
    let gap = (left.finished_at().unwrap() - right.finished_at().unwrap()).abs();
    assert!(gap < 0.1, "the arms finished {gap} s apart");
    let joints_error = |record: &hebbian_contracts::CommandRecord| {
        record
            .achieved
            .as_ref()
            .and_then(|achieved| achieved.joint_error_radians)
            .unwrap()
    };
    assert!(joints_error(&left) < 0.1 && joints_error(&right) < 0.1);
}

#[test]
fn when_one_arm_of_a_group_ends_early_the_others_stop_and_hold() {
    let mut arms = two_arms();
    move_arms(
        &arms.left,
        "group",
        [&json!([1.5, 0.0]), &json!([0.0, 1.5])],
    );
    arms.left
        .read_until(|event| matches!(event, Event::Reply { .. }));
    arms.right.read_until(in_mode(ArmMode::Moving));
    // Stopping the left arm alone ends its part, and the right arm's with it.
    arms.left.send_command(None, &json!({"kind": "stop"}));
    let left = ended_record(&mut arms.left);
    let right = ended_record(&mut arms.right);
    assert_eq!(left.outcome(), Some(CommandOutcome::Stopped));
    assert_eq!(right.outcome(), Some(CommandOutcome::Stopped), "{right:?}");
    assert!(
        right
            .detail()
            .is_some_and(|detail| detail.contains("left ended stopped")),
        "{right:?}"
    );
    arms.right.read_until(in_mode(ArmMode::Holding));
}

#[test]
fn a_trip_on_one_arm_of_a_group_stops_the_other() {
    let mut arms = two_arms();
    move_arms(
        &arms.left,
        "group",
        [&json!([1.5, 0.0]), &json!([0.0, 1.5])],
    );
    arms.left
        .read_until(|event| matches!(event, Event::Reply { .. }));
    arms.right.read_until(in_mode(ArmMode::Moving));
    arms.faults[0].store(true, Ordering::Relaxed);
    let left = ended_record(&mut arms.left);
    let right = ended_record(&mut arms.right);
    assert_eq!(left.outcome(), Some(CommandOutcome::Tripped), "{left:?}");
    assert_eq!(right.outcome(), Some(CommandOutcome::Stopped), "{right:?}");
    assert!(
        right
            .detail()
            .is_some_and(|detail| detail.contains("left ended tripped")),
        "{right:?}"
    );
}

/// A fake simulator for two arms sharing one scene: it waits for every arm's
/// step, advances its joints toward the targets, then answers each arm. It stops
/// answering after `answer_steps` steps, to stand in for a stalled simulation.
fn fake_simulator(
    directory: &Path,
    answer_steps: usize,
) -> (std::sync::mpsc::Sender<()>, std::thread::JoinHandle<()>) {
    let directory = directory.to_path_buf();
    let (shutdown_sender, shutdown_receiver) = std::sync::mpsc::channel();
    let simulator_thread = std::thread::spawn(move || {
        let names = ["left", "right"];
        let mut streams = Vec::new();
        for name in names {
            let path = directory.join(format!("{name}.plant.sock"));
            let deadline = Instant::now() + Duration::from_secs(5);
            let stream = loop {
                if let Ok(stream) = UnixStream::connect(&path) {
                    break stream;
                }
                assert!(Instant::now() < deadline, "no plant socket for {name}");
                std::thread::sleep(Duration::from_millis(10));
            };
            let mut writer = stream.try_clone().unwrap();
            let hello = json!({
                "protocol_version": 1, "joint_names": ["a", "b"],
                "state": {"positions": [0.0, 0.0], "velocities": [0.0, 0.0], "efforts": [0.0, 0.0]},
            });
            writeln!(writer, "{hello}").unwrap();
            streams.push((writer, BufReader::new(stream), vec![0.0_f64, 0.0]));
        }
        for _ in 0..answer_steps {
            let mut steps = Vec::new();
            for (_, reader, _) in &mut streams {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    return;
                }
                steps.push(serde_json::from_str::<Value>(&line).unwrap());
            }
            for ((writer, _, positions), step) in streams.iter_mut().zip(&steps) {
                let seconds = step["seconds"].as_f64().unwrap();
                for (position, target) in positions
                    .iter_mut()
                    .zip(step["positions"].as_array().unwrap())
                {
                    *position += (target.as_f64().unwrap() - *position) * (seconds / 0.05).min(1.0);
                }
                let state = json!({"positions": positions, "velocities": [0.0, 0.0], "efforts": [0.0, 0.0]});
                writeln!(writer, "{state}").unwrap();
            }
        }
        // Silence: keep the sockets open and answer nothing.
        let _ = shutdown_receiver.recv();
    });
    (shutdown_sender, simulator_thread)
}

fn sim_config(reply_timeout_seconds: f64) -> ExecutorConfig {
    let mut config = test_config().into_input();
    config.sim_plant = Some(hebbian_robot_use_server::config::SimPlantConfig {
        connect_timeout_seconds: 5.0,
        reply_timeout_seconds,
        real_time: true,
    });
    ExecutorConfig::try_from(config).unwrap()
}

fn sim_setups(
    directory: &Path,
    reply_timeout_seconds: f64,
) -> Vec<ArmSetup<hebbian_robot_use_server::sim_plant::SocketPlant>> {
    use hebbian_robot_use_server::sim_plant::{SimTimeouts, SocketPlant};
    let control = control();
    let listeners: Vec<_> = ["left", "right"]
        .into_iter()
        .map(|name| {
            let path = directory.join(format!("{name}.plant.sock"));
            (name, std::os::unix::net::UnixListener::bind(path).unwrap())
        })
        .collect();
    listeners
        .into_iter()
        .map(|(name, listener)| {
            let config = sim_config(reply_timeout_seconds);
            let plant = SocketPlant::accept(
                &listener,
                &config.driven_joints,
                config.joint_count,
                SimTimeouts {
                    connect: Duration::from_secs(5),
                    reply: Duration::from_secs_f64(reply_timeout_seconds),
                    real_time: true,
                },
                &AtomicBool::new(false),
            )
            .unwrap();
            let link = Link::new(&config, Arc::clone(&control)).unwrap();
            ArmSetup {
                name: name.to_owned(),
                config,
                plant,
                link: Some(link),
            }
        })
        .collect()
}

#[test]
fn arms_in_one_simulator_follow_commands_with_time_driven_by_its_replies() {
    let directory = std::env::temp_dir().join(format!("hebbian-sim-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let (simulator_shutdown, simulator) = fake_simulator(&directory, 100_000);
    let mut setups = sim_setups(&directory, 2.0);
    let left_handle = setups[0].link.as_ref().unwrap().handle();
    let shutdown = Arc::new(AtomicBool::new(false));
    let stop = Arc::clone(&shutdown);
    let loop_thread = std::thread::spawn(move || {
        let reports = run_arms(&mut setups, Some(Duration::from_secs(3)), &stop);
        for setup in &mut setups {
            assert_eq!(setup.config.trace_directory, test_config().trace_directory);
            if let Some(link) = setup.link.take() {
                link.close();
            }
        }
        reports
    });
    let mut client = LinkClient::connect(&left_handle);
    client.send_command(None, &json!({"kind": "enable"}));
    client.read_until(in_mode(ArmMode::Holding));
    client.send_command(
        None,
        &json!({"kind": "stream_joints", "positions": [0.4, -0.2]}),
    );
    client.read_until(|event| {
        matches!(event, Event::State(Sample { positions, .. })
            if (positions[0] - 0.4).abs() < 0.02 && (positions[1] + 0.2).abs() < 0.02)
    });
    shutdown.store(true, Ordering::Relaxed);
    let reports = loop_thread.join().unwrap();
    assert!(reports.iter().all(|(_, report)| report.trip.is_none()));
    for (name, report) in &reports {
        let expected_directory = Path::new(&test_config().trace_directory).join(name);
        assert_eq!(
            report.trace.as_ref().unwrap().path.parent(),
            Some(expected_directory.as_path())
        );
    }
    drop(simulator_shutdown);
    simulator.join().unwrap();
}

#[test]
fn a_simulator_that_stops_answering_trips_the_arms_instead_of_hanging_the_loop() {
    let directory = std::env::temp_dir().join(format!("hebbian-sim-stall-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let (simulator_shutdown, simulator) = fake_simulator(&directory, 20);
    let mut setups = sim_setups(&directory, 0.3);
    let started = Instant::now();
    let reports = run_arms(
        &mut setups,
        Some(Duration::from_secs(10)),
        &AtomicBool::new(false),
    );
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the loop must not wait out its run time"
    );
    for setup in &mut setups {
        if let Some(link) = setup.link.take() {
            link.close();
        }
    }
    assert!(reports.iter().all(|(_, report)| {
        report
            .trip
            .as_deref()
            .is_some_and(|reason| reason.contains("did not answer"))
    }));
    drop(simulator_shutdown);
    simulator.join().unwrap();
}

/// A simulated arm whose bus can be switched off, and that records its release.
struct SwitchedBusPlant {
    inner: SimulatedPlant,
    bus_down: Arc<AtomicBool>,
    released: Arc<AtomicBool>,
}

impl Plant for SwitchedBusPlant {
    fn read_feedback_into(&mut self, feedback: &mut Feedback) -> anyhow::Result<()> {
        anyhow::ensure!(!self.bus_down.load(Ordering::Relaxed), "bus down");
        self.inner.read_feedback_into(feedback)
    }

    fn command(&mut self, positions: &[f64], seconds: f64) -> anyhow::Result<()> {
        self.inner.command(positions, seconds)
    }

    fn command_gripper(&mut self, angle: f64) -> anyhow::Result<()> {
        self.inner.command_gripper(angle)
    }

    fn preflight(&mut self) -> anyhow::Result<()> {
        self.inner.preflight()
    }

    fn enable(&mut self) -> anyhow::Result<()> {
        self.inner.enable()
    }

    fn release(&mut self) -> anyhow::Result<()> {
        self.released.store(true, Ordering::Relaxed);
        self.inner.release()
    }
}

#[test]
fn bus_outages_do_not_release_torque() {
    let config = test_config();
    let link = Link::new(&config, control()).unwrap();
    let handle = link.handle();
    let bus_down = Arc::new(AtomicBool::new(false));
    let released = Arc::new(AtomicBool::new(false));
    let mut plant = SwitchedBusPlant {
        inner: SimulatedPlant::at(vec![0.0, 0.0], 0.02),
        bus_down: Arc::clone(&bus_down),
        released: Arc::clone(&released),
    };
    let loop_thread = std::thread::spawn(move || {
        let report = run(
            &mut plant,
            Some(&link),
            &config,
            Some(Duration::from_secs(3)),
            &AtomicBool::new(false),
        );
        link.close();
        report
    });
    let mut client = LinkClient::connect(&handle);
    client.send_command(None, &json!({"kind": "enable"}));
    client.read_until(in_mode(ArmMode::Holding));

    bus_down.store(true, Ordering::Relaxed);
    std::thread::sleep(Duration::from_millis(150));
    bus_down.store(false, Ordering::Relaxed);
    std::thread::sleep(Duration::from_millis(100));
    assert!(
        !released.load(Ordering::Relaxed),
        "a short outage keeps torque"
    );

    bus_down.store(true, Ordering::Relaxed);
    let report = loop_thread.join().unwrap();
    assert!(
        !released.load(Ordering::Relaxed),
        "a long outage keeps torque"
    );
    assert!(
        report
            .trip
            .is_some_and(|reason| reason.contains("bus down"))
    );
}

#[test]
fn a_signal_returns_the_arm_to_rest_and_releases_it_before_the_run_ends() {
    let config = test_config();
    let link = Link::new(&config, control()).unwrap();
    let handle = link.handle();
    let released = Arc::new(AtomicBool::new(false));
    let signalled = Arc::new(AtomicBool::new(false));
    let mut plant = SwitchedBusPlant {
        inner: SimulatedPlant::at(vec![0.0, 0.0], 0.02),
        bus_down: Arc::new(AtomicBool::new(false)),
        released: Arc::clone(&released),
    };
    let signal = Arc::clone(&signalled);
    let loop_thread = std::thread::spawn(move || {
        let report = run(
            &mut plant,
            Some(&link),
            &config,
            Some(Duration::from_secs(30)),
            &signal,
        );
        link.close();
        report
    });
    let mut client = LinkClient::connect(&handle);
    client.send_command(None, &json!({"kind": "enable"}));
    client.read_until(in_mode(ArmMode::Holding));

    let started = Instant::now();
    signalled.store(true, Ordering::Relaxed);
    loop_thread.join().unwrap();
    assert!(started.elapsed() < Duration::from_secs(10));
    assert!(released.load(Ordering::Relaxed));
}

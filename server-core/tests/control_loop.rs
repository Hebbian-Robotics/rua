#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    reason = "This integration-test target may abort on failed setup or unexpected outcomes."
)]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use hebbian_robot_use_core::config::ControllerConfig;
use hebbian_robot_use_core::control_link::Link;
use hebbian_robot_use_core::control_loop::{ArmSetup, run, run_arms};
use hebbian_robot_use_core::lease::{LEASE, Lease};
use hebbian_robot_use_core::plant::{Feedback, NoPhysicsStandIn, Pacing, Plant, PlantWritePermit};
use serde_json::{Value, json};

mod support;
use hebbian_contracts::{
    ArmMode, ArmMove, Command, CommandOutcome, CommandRecord, CornerPlan, Event, Sample, Segment,
};
use hebbian_robot_use_core::controller_transport::{ControllerAnswer, ControllerRequest};
use support::{LinkClient, RunningArms};

const CONFIG: &str = include_str!("support/two_joint_config.json");

fn control() -> Arc<Lease> {
    Arc::new(Lease::new(LEASE))
}

fn test_config() -> ControllerConfig {
    let mut config = ControllerConfig::parse(CONFIG).unwrap().into_input();
    config.trace_directory = std::env::temp_dir()
        .join("hebbian-robot-use-server-test-traces")
        .to_string_lossy()
        .into_owned();
    ControllerConfig::try_from(config).unwrap()
}

fn in_mode(mode: ArmMode) -> impl FnMut(&Event) -> bool {
    move |event| matches!(event, Event::State(sample) if sample.mode == mode)
}

struct FaultingPlant {
    inner: NoPhysicsStandIn,
    reads_since_enable: Option<u32>,
    fault_active: Arc<AtomicBool>,
}

impl Plant for FaultingPlant {
    fn pacing(&self) -> Pacing {
        self.inner.pacing()
    }

    fn read_feedback_into(&mut self, feedback: &mut Feedback) -> anyhow::Result<()> {
        self.inner.read_feedback_into(feedback)?;
        feedback.temperatures_celsius = vec![70.0, 31.0];
        if let Some(reads) = &mut self.reads_since_enable {
            *reads += 1;
            if *reads > 2 && self.fault_active.load(Ordering::Relaxed) {
                feedback.fault = Some("1: ok 70C".to_owned());
            }
        }
        Ok(())
    }

    fn command(
        &mut self,
        permit: &PlantWritePermit,
        positions: &[f64],
        seconds: f64,
    ) -> anyhow::Result<()> {
        self.inner.command(permit, positions, seconds)
    }

    fn command_gripper(&mut self, permit: &PlantWritePermit, angle: f64) -> anyhow::Result<()> {
        self.inner.command_gripper(permit, angle)
    }

    fn preflight(&mut self) -> anyhow::Result<()> {
        self.inner.preflight()
    }

    fn enable(&mut self, permit: &PlantWritePermit) -> anyhow::Result<()> {
        self.reads_since_enable = Some(0);
        self.inner.enable(permit)
    }

    fn release(&mut self, permit: &PlantWritePermit) -> anyhow::Result<()> {
        self.inner.release(permit)
    }
}

#[test]
fn a_tripped_loop_trips_once_and_keeps_reporting_the_fault() {
    let config = test_config();
    let link = Link::new(&config, control()).unwrap();
    let handle = link.handle();
    let mut client = LinkClient::connect(&handle);
    let shutdown_requested = Arc::new(AtomicBool::new(false));
    let loop_shutdown_requested = Arc::clone(&shutdown_requested);
    let fault_active = Arc::new(AtomicBool::new(true));
    let plant_fault_active = Arc::clone(&fault_active);
    let loop_thread = std::thread::spawn(move || {
        let mut plant = FaultingPlant {
            inner: NoPhysicsStandIn::at(vec![0.0, 0.0], 0.02),
            reads_since_enable: None,
            fault_active: plant_fault_active,
        };
        let report = run(
            &mut plant,
            &link,
            &config,
            Some(Duration::from_secs(10)),
            &loop_shutdown_requested,
        );
        link.close();
        report
    });

    client.command(Command::Enable);
    // WHY: Wait for repeated faulted observations, rather than assuming a number of
    // cycles fits inside a wall-clock interval on a shared CI runner.
    let mut events = Vec::new();
    let mut faulted_samples = 0;
    while faulted_samples < 3 {
        let event = client.next_frame().event;
        if matches!(&event, Event::State(sample) if sample.fault.is_some()) {
            faulted_samples += 1;
        }
        events.push(event);
    }
    fault_active.store(false, Ordering::Relaxed);
    client.read_until(|event| {
        matches!(event, Event::State(sample) if sample.fault.is_none() && sample.active_trip.is_none())
    });
    fault_active.store(true, Ordering::Relaxed);
    client.read_until(|event| matches!(event, Event::Tripped { reason } if reason == "1: ok 70C"));
    shutdown_requested.store(true, Ordering::Relaxed);
    let report = loop_thread.join().unwrap();
    events.extend(client.drain_events());

    assert_eq!(report.trip.as_deref(), Some("1: ok 70C"));
    let trips = events
        .iter()
        .filter(|event| matches!(event, Event::Tripped { .. }))
        .count();
    assert_eq!(
        trips, 1,
        "a held arm is not tripped again while the fault persists"
    );
    let states: Vec<&Sample> = events
        .iter()
        .filter_map(|event| match event {
            Event::State(sample) => Some(sample),
            _ => None,
        })
        .collect();
    let first_faulted = states
        .iter()
        .position(|state| state.fault.is_some())
        .expect("the fault shows");
    for state in &states[first_faulted..] {
        assert_eq!(state.fault.as_deref(), Some("1: ok 70C"));
        assert_eq!(state.temperatures_celsius, vec![70.0, 31.0]);
    }
    for state in states[first_faulted..].iter().take(3) {
        assert_eq!(state.active_trip.as_deref(), Some("1: ok 70C"));
    }
}

#[test]
fn the_loop_runs_until_shutdown_is_requested() {
    let config = test_config();
    let shutdown_requested = Arc::new(AtomicBool::new(false));
    let loop_shutdown_requested = Arc::clone(&shutdown_requested);
    let link = Link::new(&config, control()).unwrap();
    let mut client = LinkClient::connect(&link.handle());
    let loop_thread = std::thread::spawn(move || {
        let mut plant = NoPhysicsStandIn::at(vec![0.0, 0.0], 0.02);
        let report = run(&mut plant, &link, &config, None, &loop_shutdown_requested);
        link.close();
        report
    });
    client.read_until(in_mode(ArmMode::Disabled));
    assert!(!loop_thread.is_finished(), "the loop awaits shutdown");
    shutdown_requested.store(true, Ordering::Relaxed);
    let report = loop_thread.join().unwrap();
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
        let mut plant = NoPhysicsStandIn::at(vec![2.02, 0.0], 0.02);
        let report = run(
            &mut plant,
            &link,
            &config,
            Some(Duration::from_secs(4)),
            &loop_shutdown_requested,
        );
        link.close();
        report
    });
    let mut client = LinkClient::connect(&handle);
    client.command(Command::Enable);
    client.read_until(in_mode(ArmMode::Holding));
    let accepted = client.command(Command::Move {
        label: "recover".into(),
        plan: straight_plan("recover", [2.02, 0.0], [1.8, 0.0]),
        joint_speed: hebbian_contracts::JointSpeed::new(1.0).unwrap(),
        path_limited: None,
        pose_limited: None,
    });
    let outcome = client.read_until(|event| {
        matches!(event, Event::Record(record) if record.command_id == accepted.command_id && record.outcome().is_some())
    });
    assert!(
        matches!(outcome, Event::Record(ref record) if record.outcome() == Some(CommandOutcome::Completed)),
        "{outcome:?}"
    );
    shutdown_requested.store(true, Ordering::Relaxed);
    assert!(loop_thread.join().unwrap().trip.is_none());
}

struct TwoArms {
    left: LinkClient,
    right: LinkClient,
    faults: [Arc<AtomicBool>; 2],
    _running_arms: RunningArms,
}

struct SwitchedFaultPlant {
    inner: NoPhysicsStandIn,
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

    fn command(
        &mut self,
        permit: &PlantWritePermit,
        positions: &[f64],
        seconds: f64,
    ) -> anyhow::Result<()> {
        self.inner.command(permit, positions, seconds)
    }

    fn command_gripper(&mut self, permit: &PlantWritePermit, angle: f64) -> anyhow::Result<()> {
        self.inner.command_gripper(permit, angle)
    }

    fn preflight(&mut self) -> anyhow::Result<()> {
        self.inner.preflight()
    }

    fn enable(&mut self, permit: &PlantWritePermit) -> anyhow::Result<()> {
        self.inner.enable(permit)
    }

    fn release(&mut self, permit: &PlantWritePermit) -> anyhow::Result<()> {
        self.inner.release(permit)
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
            let mut config = test_config().into_input();
            config.urdf =
                concat!(env!("CARGO_MANIFEST_DIR"), "/tests/support/two_joint.urdf").into();
            config.base_pose = Some(hebbian_contracts::BasePose {
                position_meters: [0.0, if *name == "right" { 1.0 } else { 0.0 }, 0.0],
                yaw_radians: 0.0,
            });
            config.self_collision = Some(hebbian_contracts::SelfCollisionConfig {
                penetration_tolerance_meters: 0.003,
                allowed_collision_pairs: Vec::new(),
            });
            let config = ControllerConfig::try_from(config).unwrap();
            let link = Link::new(&config, Arc::clone(&control)).unwrap();
            ArmSetup::new(
                (*name).to_owned(),
                config,
                SwitchedFaultPlant {
                    inner: NoPhysicsStandIn::at(vec![0.0, 0.0], 0.02),
                    fault: Arc::clone(fault),
                },
                link,
            )
        })
        .collect();
    let handles: Vec<_> = setups.iter().map(|setup| setup.link.handle()).collect();
    let running_arms = RunningArms::start(setups);
    let mut left = LinkClient::connect(&handles[0]);
    let mut right = LinkClient::connect(&handles[1]);
    for client in [&mut left, &mut right] {
        client.command(Command::Enable);
        client.read_until(in_mode(ArmMode::Holding));
    }
    TwoArms {
        left,
        right,
        faults,
        _running_arms: running_arms,
    }
}

fn straight_plan(label: &str, start: [f64; 2], target: [f64; 2]) -> CornerPlan {
    CornerPlan {
        peak_speed: 1.0,
        segments: vec![Segment {
            pass_through: None,
            label: label.into(),
            corners: vec![start.to_vec(), target.to_vec()],
            minimum_duration_seconds: None,
            target_position: None,
            target_quaternion: None,
        }],
    }
}

fn move_arms(client: &LinkClient, targets: [[f64; 2]; 2]) -> Vec<CommandRecord> {
    let moves = ["left", "right"]
        .into_iter()
        .zip(targets)
        .map(|(arm, target)| ArmMove {
            arm: arm.into(),
            plan: straight_plan("group_target", [0.0, 0.0], target),
            joint_speed: hebbian_contracts::JointSpeed::new(1.0).unwrap(),
            path_limited: None,
            pose_limited: None,
        })
        .map(|part| {
            hebbian_robot_use_core::action_step::PreparedArmAction::from_group_move(
                "move_arms",
                part,
            )
        })
        .collect();
    match client.submit(ControllerRequest::ActionStep {
        actions: moves,
        trace_context: None,
    }) {
        ControllerAnswer::ActionStep(records) => {
            records.into_iter().map(|entry| entry.record).collect()
        }
        answer => panic!("group move was not accepted: {answer:?}"),
    }
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
fn a_grouped_move_takes_every_arm_to_its_target() {
    let mut arms = two_arms();
    let records = move_arms(&arms.left, [[0.6, 0.0], [0.0, -0.3]]);
    assert_eq!(records.len(), 2);
    assert!(records[0].group_id().is_some());
    assert_eq!(records[0].group_id(), records[1].group_id());

    let left = ended_record(&mut arms.left);
    let right = ended_record(&mut arms.right);
    assert_eq!(left.outcome(), Some(CommandOutcome::Completed), "{left:?}");
    assert_eq!(
        right.outcome(),
        Some(CommandOutcome::Completed),
        "{right:?}"
    );
    let joints_error = |record: &hebbian_contracts::CommandRecord| {
        record
            .achieved()
            .and_then(|achieved| achieved.joint_error_radians)
            .unwrap()
    };
    assert!(joints_error(&left) < 0.1 && joints_error(&right) < 0.1);
}

#[test]
fn when_one_arm_of_a_group_ends_early_the_others_stop_and_hold() {
    let mut arms = two_arms();
    move_arms(&arms.left, [[1.5, 0.0], [0.0, 1.5]]);
    arms.right.read_until(in_mode(ArmMode::Moving));
    arms.left.command(Command::Stop);
    let left = ended_record(&mut arms.left);
    let right = ended_record(&mut arms.right);
    assert_eq!(left.outcome(), Some(CommandOutcome::Stopped));
    assert_eq!(right.outcome(), Some(CommandOutcome::Stopped), "{right:?}");
    assert!(
        right
            .detail()
            .is_some_and(|detail| detail.contains("left requested stop")),
        "{right:?}"
    );
    arms.right.read_until(in_mode(ArmMode::Holding));
}

#[test]
fn a_trip_on_one_arm_of_a_group_stops_the_other() {
    let mut arms = two_arms();
    move_arms(&arms.left, [[1.5, 0.0], [0.0, 1.5]]);
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
        // WHY: Silence: keep the sockets open and answer nothing.
        let _ = shutdown_receiver.recv();
    });
    (shutdown_sender, simulator_thread)
}

fn sim_config(reply_timeout_seconds: f64) -> ControllerConfig {
    let mut config = test_config().into_input();
    config.sim_plant = Some(hebbian_robot_use_core::config::SimPlantConfig {
        connect_timeout_seconds: 5.0,
        reply_timeout_seconds,
        real_time: true,
    });
    ControllerConfig::try_from(config).unwrap()
}

fn sim_setups(
    directory: &Path,
    reply_timeout_seconds: f64,
) -> Vec<ArmSetup<hebbian_robot_use_core::sim_plant::SocketPlant>> {
    use hebbian_robot_use_core::sim_plant::{SimTimeouts, SocketPlant};
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
                || false,
            )
            .unwrap();
            let link = Link::new(&config, Arc::clone(&control)).unwrap();
            ArmSetup::new(name.to_owned(), config, plant, link)
        })
        .collect()
}

#[test]
fn arms_in_one_simulator_follow_commands_with_time_driven_by_its_replies() {
    let directory = std::env::temp_dir().join(format!("hebbian-sim-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let (simulator_shutdown, simulator) = fake_simulator(&directory, 100_000);
    let mut setups = sim_setups(&directory, 2.0);
    let mut left_config = setups[0].config.clone().into_input();
    left_config.enable_checks = Some(hebbian_contracts::EnableChecks {
        preflight_drift_limit_radians: 0.02,
        enable_drift_limit_radians: 0.05,
    });
    setups[0].config = ControllerConfig::try_from(left_config).unwrap();
    let left_handle = setups[0].link.handle();
    let shutdown = Arc::new(AtomicBool::new(false));
    let stop = Arc::clone(&shutdown);
    let loop_thread = std::thread::spawn(move || {
        let reports = run_arms(&mut setups, None, &stop);
        for setup in setups {
            assert_eq!(setup.config.trace_directory, test_config().trace_directory);
            setup.link.close();
        }
        reports
    });
    let mut client = LinkClient::connect(&left_handle);
    client.command(Command::Enable);
    client.read_until(in_mode(ArmMode::Holding));
    client.command(Command::StreamJoints {
        positions: vec![0.4, -0.2],
        gripper_angle: None,
        joint_speed: None,
        stream_command_id: None,
    });
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
    for setup in setups {
        setup.link.close();
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

struct SwitchedBusPlant {
    inner: NoPhysicsStandIn,
    bus_down: Arc<AtomicBool>,
    released: Arc<AtomicBool>,
}

impl Plant for SwitchedBusPlant {
    // CONSTRAINT: Model a hardware bus: simulator pacing deliberately treats a lost peer
    // as terminal instead of exercising hardware reconnection and torque hold.
    fn read_feedback_into(&mut self, feedback: &mut Feedback) -> anyhow::Result<()> {
        anyhow::ensure!(!self.bus_down.load(Ordering::Relaxed), "bus down");
        self.inner.read_feedback_into(feedback)
    }

    fn command(
        &mut self,
        permit: &PlantWritePermit,
        positions: &[f64],
        seconds: f64,
    ) -> anyhow::Result<()> {
        self.inner.command(permit, positions, seconds)
    }

    fn command_gripper(&mut self, permit: &PlantWritePermit, angle: f64) -> anyhow::Result<()> {
        self.inner.command_gripper(permit, angle)
    }

    fn preflight(&mut self) -> anyhow::Result<()> {
        self.inner.preflight()
    }

    fn enable(&mut self, permit: &PlantWritePermit) -> anyhow::Result<()> {
        self.inner.enable(permit)
    }

    fn release(&mut self, permit: &PlantWritePermit) -> anyhow::Result<()> {
        self.released.store(true, Ordering::Relaxed);
        self.inner.release(permit)
    }
}

#[test]
fn bus_outages_do_not_release_torque() {
    let mut configuration = test_config().into_input();
    // WHY: Exercise bus recovery, without an unrelated host scheduling trip winning
    // the race on a shared CI runner. Production profiles are unchanged.
    configuration.loop_deadline_seconds = 5.0;
    let config = ControllerConfig::try_from(configuration).unwrap();
    let link = Link::new(&config, control()).unwrap();
    let handle = link.handle();
    let bus_down = Arc::new(AtomicBool::new(false));
    let released = Arc::new(AtomicBool::new(false));
    let mut plant = SwitchedBusPlant {
        inner: NoPhysicsStandIn::at(vec![0.0, 0.0], 0.02),
        bus_down: Arc::clone(&bus_down),
        released: Arc::clone(&released),
    };
    let shutdown_requested = Arc::new(AtomicBool::new(false));
    let loop_shutdown_requested = Arc::clone(&shutdown_requested);
    let loop_thread = std::thread::spawn(move || {
        let report = run(
            &mut plant,
            &link,
            &config,
            Some(Duration::from_secs(30)),
            &loop_shutdown_requested,
        );
        link.close();
        report
    });
    let mut client = LinkClient::connect(&handle);
    client.command(Command::Enable);
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
    client.read_until(in_mode(ArmMode::Disconnected));
    shutdown_requested.store(true, Ordering::Relaxed);
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
        inner: NoPhysicsStandIn::at(vec![0.0, 0.0], 0.02),
        bus_down: Arc::new(AtomicBool::new(false)),
        released: Arc::clone(&released),
    };
    let signal = Arc::clone(&signalled);
    let loop_thread = std::thread::spawn(move || {
        let report = run(
            &mut plant,
            &link,
            &config,
            Some(Duration::from_secs(30)),
            &signal,
        );
        link.close();
        report
    });
    let mut client = LinkClient::connect(&handle);
    client.command(Command::Enable);
    client.read_until(in_mode(ArmMode::Holding));

    let started = Instant::now();
    signalled.store(true, Ordering::Relaxed);
    loop_thread.join().unwrap();
    assert!(started.elapsed() < Duration::from_secs(10));
    assert!(released.load(Ordering::Relaxed));
}

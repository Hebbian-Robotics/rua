#![allow(
    clippy::disallowed_methods,
    reason = "Preview tests command only local simulated plants."
)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    reason = "This integration-test target may abort on failed setup or unexpected outcomes."
)]

// SAFETY: Preview uses a separate model and controller state: it commands no physical arm and writes no command records.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use hebbian_robot_use_server::arm::ReachGoal;
use hebbian_robot_use_server::config::ExecutorConfig;
use hebbian_robot_use_server::control_link::Link;
use hebbian_robot_use_server::lease::{LEASE, Lease};
use hebbian_robot_use_server::preview::{Preview, PreviewOutcome, PreviewRequest};
use hebbian_robot_use_server::robot::Robot;
use serde_json::{Value, json};

mod support;
use hebbian_contracts::{ArmMode, CommandOutcome, Event, ProjectionOutcome};
use support::LinkClient;

const JOINTS: [&str; 6] = [
    "shoulder_pan",
    "shoulder_lift",
    "elbow_flex",
    "wrist_flex",
    "wrist_roll",
    "gripper",
];
const START: [f64; 5] = [0.0, -0.5, 0.8, 0.6, 0.0];
const CHECKPOINT_SHA256: &str = "89f9f5a47866d4e2761373e6036adde6277c400f1942ff7f74bd26fda6f7eb63";
const MAX_TARGET_LEAD_RADIANS: f64 = 0.12;
const TOOL_LINK: &str = "gripper_frame_link";

fn bundled_urdf() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("robots/so101/robot-soft-gripper-camera.urdf")
}

fn assets() -> Option<PathBuf> {
    // CONSTRAINT: These optional tests require an explicitly selected ONNX runtime as well
    // as the private policy assets; the standard build does not bundle one.
    std::env::var_os("ORT_DYLIB_PATH")?;
    let directory = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()?
        .join(".cache/so101-policy-assets");
    ["policy.onnx", "tool-frame.json"]
        .iter()
        .all(|name| directory.join(name).is_file())
        .then_some(directory)
}

fn control() -> Arc<Lease> {
    Arc::new(Lease::new(LEASE))
}

fn config(assets: &Path) -> ExecutorConfig {
    config_accepting_target_lead(assets, MAX_TARGET_LEAD_RADIANS)
}

fn config_accepting_target_lead(assets: &Path, max_target_lead_radians: f64) -> ExecutorConfig {
    let text = json!({
        "urdf": bundled_urdf(),
        "driven_joints": JOINTS,
        "surface_guard": null,
        "self_collision": null,
        "feetech": null,
        "tool_link": TOOL_LINK,
        "policy": {
            "reach_deadline_seconds":8.0,"settle_dwell_seconds":0.1,
            "settled_velocity_radians_per_second":0.15,"preview_lag_seconds":0.05,
            "onnx": assets.join("policy.onnx"),
            "tool_frame_fixture": assets.join("tool-frame.json"),
            "checkpoint_sha256": std::env::var("SO101_POLICY_TEST_CHECKPOINT_SHA256")
                .unwrap_or_else(|_| CHECKPOINT_SHA256.to_owned()),
            "tool_link": TOOL_LINK,
            "max_target_lead_radians": max_target_lead_radians,
            "position_tolerance_meters": 0.02, "orientation_tolerance_radians": 0.1,
        },
        "trace_directory": std::env::temp_dir().join("hebbian-robot-use-server-preview-traces"),
        "motion": {
            "acceleration_limit": 5.0, "jerk_limit": 20.0,
            "settled_error_radians": 0.04, "settle_dwell_seconds": 0.3,
            "settle_timeout_seconds": 8.0
        },
        "joint_count": 5, "frequency_hertz": 50.0, "loop_deadline_seconds": 0.1,
        "lower_limits": [-1.91986, -1.74533, -1.69, -1.65806, -2.74385],
        "upper_limits": [1.91986, 1.74533, 1.69, 1.65806, 2.84121],
        "joint_effort_limits": [80.0, 80.0, 80.0, 80.0, 80.0],
        "tracking_following_lag_seconds": 0.05, "tracking_static_margin_radians": 0.1,
        "max_joint_speed": 2.5, "default_joint_speed": 2.5,
        "stream_goal_timeout_seconds": 5.0,
        "joint_limit_recovery_tolerance_radians": 0.0349, "jaw_closing_axis": "x",
    })
    .to_string();
    ExecutorConfig::parse(&text).unwrap()
}

fn goal_near_start(offset: [f64; 3]) -> ReachGoal {
    let joints: Vec<String> = JOINTS.iter().map(|name| (*name).to_owned()).collect();
    let robot = Robot::load(&bundled_urdf(), &joints).unwrap();
    let mut full = START.to_vec();
    full.push(0.0);
    let pose = robot.link_pose(TOOL_LINK, &full).unwrap();
    let position = pose.translation.vector;
    let rotation = pose.rotation;
    ReachGoal {
        position: [
            position.x + offset[0],
            position.y + offset[1],
            position.z + offset[2],
        ],
        quaternion: [rotation.i, rotation.j, rotation.k, rotation.w],
        position_tolerance_meters: 0.02,
        orientation_tolerance_radians: 0.1,
    }
}

fn request(goal: ReachGoal) -> PreviewRequest {
    PreviewRequest {
        goal,
        joints: START.to_vec(),
        gripper_angle: Some(0.0),
    }
}

#[test]
fn a_reachable_pose_settles_and_the_same_question_gets_the_same_answer() {
    let Some(assets) = assets() else {
        eprintln!("skipped: the policy assets or ORT_DYLIB_PATH are unavailable");
        return;
    };
    let mut preview = Preview::new(&config(&assets)).unwrap().unwrap();
    let asked = request(goal_near_start([0.06, 0.0, 0.0]));
    let started = Instant::now();
    let first = preview.reach(&asked).unwrap();
    eprintln!(
        "rehearsed {} cycles ({:.2} s simulated) in {:?}",
        first.steps,
        first.simulated_seconds,
        started.elapsed()
    );
    assert_eq!(first.outcome, PreviewOutcome::Settled, "{first:?}");
    assert!(first.position_error_meters <= 0.02, "{first:?}");
    assert_eq!(preview.reach(&asked).unwrap(), first);
}

#[test]
fn a_model_whose_action_step_exceeds_the_accepted_target_lead_is_refused() {
    let Some(assets) = assets() else {
        eprintln!("skipped: the policy assets or ORT_DYLIB_PATH are unavailable");
        return;
    };
    let error = Preview::new(&config_accepting_target_lead(&assets, 0.04))
        .err()
        .unwrap();
    assert!(
        error.to_string().contains("max_target_lead_radians"),
        "{error:#}"
    );
}

#[test]
fn a_pose_beyond_reach_ends_once_the_arm_stops_closing_on_it() {
    let Some(assets) = assets() else {
        eprintln!("skipped: the policy assets or ORT_DYLIB_PATH are unavailable");
        return;
    };
    let mut preview = Preview::new(&config(&assets)).unwrap().unwrap();
    let started = Instant::now();
    let ended = preview
        .reach(&request(goal_near_start([1.5, 0.0, 0.0])))
        .unwrap();
    eprintln!(
        "worst case: {} cycles ({:.2} s simulated) in {:?}",
        ended.steps,
        ended.simulated_seconds,
        started.elapsed()
    );
    assert_eq!(ended.outcome, PreviewOutcome::TimedOut);
    assert!(ended.reason.is_some());
    // WHY: Once the arm rests short of the pose, the rest of the 8 s deadline would end at
    // the same joints, and a drag waits on every rehearsal.
    assert!(ended.simulated_seconds < 4.0, "{ended:?}");
}

#[test]
fn a_wrong_joint_count_is_refused() {
    let Some(assets) = assets() else {
        eprintln!("skipped: the policy assets or ORT_DYLIB_PATH are unavailable");
        return;
    };
    let mut preview = Preview::new(&config(&assets)).unwrap().unwrap();
    let mut asked = request(goal_near_start([0.0, 0.0, 0.0]));
    asked.joints.pop();
    assert!(preview.reach(&asked).is_err());
}

#[test]
fn a_rehearsal_over_the_link_leaves_the_arm_and_its_records_alone() {
    let Some(assets) = assets() else {
        eprintln!("skipped: the policy assets or ORT_DYLIB_PATH are unavailable");
        return;
    };
    let link = Link::new(&config(&assets), control()).unwrap();
    let mut client = LinkClient::connect(&link.handle());
    let goal = goal_near_start([0.06, 0.0, 0.0]);
    let asked: Value = json!({
        "request_id": "ghost-1",
        "request": {
            "kind": "project_pose",
            "position": goal.position,
            "orientation": {"kind": "quaternion", "quaternion": goal.quaternion},
            "joints": START,
            "gripper_angle": 0.0,
        }
    });
    client.send_line(&asked.to_string());
    let Event::Reply {
        accepted,
        projection,
        ..
    } = client.read_until(|event| matches!(event, Event::Reply { .. }))
    else {
        unreachable!()
    };
    assert!(accepted);
    let outcome = projection.unwrap().outcome;
    assert!(
        matches!(
            outcome,
            ProjectionOutcome::Projected {
                within_tolerance: true,
                ..
            }
        ),
        "the policy settles on a goal 6 cm from the start: {outcome:?}"
    );

    client.send_request(None, &json!({"kind": "snapshot"}));
    let Event::Snapshot { mode, records, .. } =
        client.read_until(|event| matches!(event, Event::Snapshot { .. }))
    else {
        unreachable!()
    };
    assert_eq!(mode, ArmMode::Disabled);
    assert!(records.is_empty(), "{records:?}");
    link.close();
}

#[test]
fn an_arm_without_a_policy_refuses_to_rehearse() {
    let Some(assets) = assets() else {
        eprintln!("skipped: the policy assets or ORT_DYLIB_PATH are unavailable");
        return;
    };
    let mut without_policy = config(&assets).into_input();
    without_policy.policy = None;
    let without_policy = ExecutorConfig::try_from(without_policy).unwrap();
    assert!(Preview::new(&without_policy).unwrap().is_none());
}

// WHY: Not a regression test: prints how the policy treats a goal rotated about the
// tool axis, for deciding how the viewer's wrist-roll slider reaches it.
// `cargo test --test preview orientation_study -- --ignored --nocapture`
#[test]
#[ignore = "a study, run by hand"]
fn orientation_study() {
    use nalgebra::UnitQuaternion;
    let Some(assets) = assets() else { return };
    let mut preview = Preview::new(&config(&assets)).unwrap().unwrap();
    let joints: Vec<String> = JOINTS.iter().map(|name| (*name).to_owned()).collect();
    let robot = Robot::load(&bundled_urdf(), &joints).unwrap();
    let pose_at = |arm: &[f64]| {
        let mut full = arm.to_vec();
        full.push(0.0);
        robot.link_pose(TOOL_LINK, &full).unwrap()
    };
    let starts: [[f64; 5]; 3] = [
        START,
        [0.4, -0.3, 0.6, 0.9, 0.3],
        [-0.5, -0.8, 1.0, 0.4, -0.4],
    ];
    for (which, start) in starts.iter().enumerate() {
        let here = pose_at(start);
        // CONSTRAINT: The roll joint's axis in the tool frame.
        let mut rolled = *start;
        rolled[4] += 0.1;
        let axis_local = (here.rotation.inverse() * pose_at(&rolled).rotation).scaled_axis() / 0.1;
        eprintln!("start {which}: roll axis in tool frame {axis_local:.3}");
        for offset in [[0.04, 0.0, 0.0], [0.0, 0.0, -0.03]] {
            for roll in [-2.0, -1.0, -0.5, 0.0, 0.5, 1.0, 2.0] {
                let rotated = here.rotation * UnitQuaternion::from_scaled_axis(axis_local * roll);
                let goal = ReachGoal {
                    position: [
                        here.translation.x + offset[0],
                        here.translation.y + offset[1],
                        here.translation.z + offset[2],
                    ],
                    quaternion: [rotated.i, rotated.j, rotated.k, rotated.w],
                    position_tolerance_meters: 0.02,
                    orientation_tolerance_radians: 0.1,
                };
                let ended = preview
                    .reach(&PreviewRequest {
                        goal,
                        joints: start.to_vec(),
                        gripper_angle: Some(0.0),
                    })
                    .unwrap();
                eprintln!(
                    "  off {offset:?} roll {roll:+.2}: {:?} pos {:.1} mm ori {:.3} rad roll_joint {:+.2} (asked {:+.2}) steps {}",
                    ended.outcome,
                    ended.position_error_meters * 1000.0,
                    ended.orientation_error_radians,
                    ended.joints[4],
                    start[4] + roll,
                    ended.steps,
                );
            }
        }
    }
}

// WHY: Not a regression test: prints how the policy treats a goal whose orientation
// the arm cannot take, with orientation not counted toward settling.
// `cargo test --test preview orientation_ignored_study -- --ignored --nocapture`
#[test]
#[ignore = "a study, run by hand"]
fn orientation_ignored_study() {
    use nalgebra::{UnitQuaternion, Vector3};
    let Some(assets) = assets() else { return };
    let mut preview = Preview::new(&config(&assets)).unwrap().unwrap();
    let joints: Vec<String> = JOINTS.iter().map(|name| (*name).to_owned()).collect();
    let robot = Robot::load(&bundled_urdf(), &joints).unwrap();
    let mut full = START.to_vec();
    full.push(0.0);
    let here = robot.link_pose(TOOL_LINK, &full).unwrap();
    for (label, turn) in [
        ("same orientation", UnitQuaternion::identity()),
        (
            "tool pitched 0.5 rad",
            UnitQuaternion::from_scaled_axis(Vector3::x() * 0.5),
        ),
        (
            "tool pitched -0.5 rad",
            UnitQuaternion::from_scaled_axis(Vector3::x() * -0.5),
        ),
        (
            "tool pitched 1.0 rad",
            UnitQuaternion::from_scaled_axis(Vector3::x() * 1.0),
        ),
        (
            "tool yawed (local y) 0.5 rad",
            UnitQuaternion::from_scaled_axis(Vector3::y() * 0.5),
        ),
        (
            "tool yawed (local y) 1.0 rad",
            UnitQuaternion::from_scaled_axis(Vector3::y() * 1.0),
        ),
        (
            "tool flipped 3.0 rad about x",
            UnitQuaternion::from_scaled_axis(Vector3::x() * 3.0),
        ),
    ] {
        let rotated = here.rotation * turn;
        for offset in [[0.04, 0.0, 0.0], [0.0, 0.0, -0.03]] {
            let goal = ReachGoal {
                position: [
                    here.translation.x + offset[0],
                    here.translation.y + offset[1],
                    here.translation.z + offset[2],
                ],
                quaternion: [rotated.i, rotated.j, rotated.k, rotated.w],
                position_tolerance_meters: 0.02,
                orientation_tolerance_radians: 0.1,
            };
            let ended = preview
                .reach(&PreviewRequest {
                    goal,
                    joints: START.to_vec(),
                    gripper_angle: Some(0.0),
                })
                .unwrap();
            eprintln!(
                "{label:30} off {offset:?}: {:?} pos {:.1} mm ori {:.3} rad steps {}",
                ended.outcome,
                ended.position_error_meters * 1000.0,
                ended.orientation_error_radians,
                ended.steps,
            );
        }
    }
}

#[test]
fn a_live_policy_reach_uses_direct_bounded_actions_and_completes_in_simulation() {
    use hebbian_robot_use_server::control_loop::run;
    use hebbian_robot_use_server::plant::SimulatedPlant;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    let Some(assets) = assets() else {
        eprintln!("skipped: the policy assets or ORT_DYLIB_PATH are unavailable");
        return;
    };
    let config = config(&assets);
    let link = Link::new(&config, control()).unwrap();
    let handle = link.handle();
    let stop = Arc::new(AtomicBool::new(false));
    let shutdown = Arc::clone(&stop);
    let loop_thread = std::thread::spawn(move || {
        let mut plant = SimulatedPlant::at(START.to_vec(), 0.05).with_gripper(0.0);
        let report = run(
            &mut plant,
            Some(&link),
            &config,
            Some(Duration::from_secs(5)),
            &shutdown,
        );
        link.close();
        report
    });
    let mut client = LinkClient::connect(&handle);
    client.send_command(None, &json!({"kind": "enable"}));
    client
        .read_until(|event| matches!(event,Event::State(sample) if sample.mode==ArmMode::Holding));
    let goal = goal_near_start([0.06, 0.0, 0.0]);
    client.send_command(
        None,
        &json!({
            "kind": "reach_pose", "label": "policy_move", "position": goal.position,
            "orientation": {"kind": "quaternion", "quaternion": goal.quaternion}
        }),
    );
    let ended = client.read_until(|event| {
        matches!(event,Event::Record(record)
        if record.label=="policy_move" && record.outcome().is_some())
    });
    let report = loop_thread.join().unwrap();
    let Event::Record(record) = ended else {
        unreachable!()
    };
    assert_eq!(
        record.outcome(),
        Some(CommandOutcome::Completed),
        "{record:?}"
    );
    assert!(report.trip.is_none(), "{:?}", report.trip);
    let trace = report.trace.unwrap();
    let text = std::fs::read_to_string(trace.path).unwrap();
    let mut active_steps = 0;
    for line in text.lines() {
        let row: Value = serde_json::from_str(line).unwrap();
        let max_step = row["command"]
            .as_array()
            .unwrap()
            .iter()
            .zip(row["positions"].as_array().unwrap())
            .map(|(command, measured)| {
                (command.as_f64().unwrap() - measured.as_f64().unwrap()).abs()
            })
            .fold(0.0_f64, f64::max);
        assert!(
            max_step <= MAX_TARGET_LEAD_RADIANS + 1e-6,
            "policy action exceeded its trained step: {max_step}"
        );
        if max_step > 0.001 {
            active_steps += 1;
        }
    }
    assert!(
        active_steps > 0,
        "the policy must actually move the simulated arm"
    );
}

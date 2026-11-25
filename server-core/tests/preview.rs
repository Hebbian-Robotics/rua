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

// INVARIANT: Preview uses a separate model and controller state: it commands no physical arm and writes no command records.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use hebbian_rua_core::arm::ReachGoal;
use hebbian_rua_core::config::ControllerConfig;
use hebbian_rua_core::control_link::Link;
use hebbian_rua_core::lease::{LEASE, Lease};
use hebbian_rua_core::preview::{Preview, PreviewOutcome, PreviewRequest};
use hebbian_rua_core::robot::Robot;
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
const MAX_TARGET_LEAD_RADIANS: f64 = 0.12;
const TOOL_LINK: &str = "gripper_frame_link";

fn bundled_urdf() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("robots/so101/robot-soft-gripper-camera.urdf")
}

fn policy_assets() -> PathBuf {
    std::env::var_os("ORT_DYLIB_PATH").expect("Set ORT_DYLIB_PATH to a compatible ONNX Runtime");
    let directory = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join(".cache/so101-policy-assets");
    assert!(
        directory.join("policy.onnx").is_file(),
        "Place the private policy checkpoint at {}",
        directory.join("policy.onnx").display()
    );
    directory
}

fn control() -> Arc<Lease> {
    Arc::new(Lease::new(LEASE))
}

fn config(assets: &Path) -> ControllerConfig {
    config_accepting_target_lead(assets, MAX_TARGET_LEAD_RADIANS)
}

fn config_accepting_target_lead(assets: &Path, max_target_lead_radians: f64) -> ControllerConfig {
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
            "tool_link": TOOL_LINK,
            "max_target_lead_radians": max_target_lead_radians,
            "position_tolerance_meters": 0.02, "orientation_tolerance_radians": 0.1,
        },
        "trace_directory": std::env::temp_dir().join("hebbian-rua-server-preview-traces"),
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
    ControllerConfig::parse(&text).unwrap()
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
        target: nalgebra::Isometry3::from_parts(
            (position + nalgebra::Vector3::from(offset)).into(),
            rotation,
        ),
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
#[ignore = "requires a private SO-101 policy checkpoint and ORT_DYLIB_PATH"]
fn a_reachable_pose_settles_and_the_same_question_gets_the_same_answer() {
    let assets = policy_assets();
    let mut preview = Preview::new(&config(&assets)).unwrap().unwrap();
    let asked = request(goal_near_start([0.06, 0.0, 0.0]));
    let first = preview.reach(&asked).unwrap();
    assert_eq!(first.outcome, PreviewOutcome::Settled, "{first:?}");
    assert!(first.position_error_meters <= 0.02, "{first:?}");
    assert_eq!(preview.reach(&asked).unwrap(), first);
}

#[test]
#[ignore = "requires a private SO-101 policy checkpoint and ORT_DYLIB_PATH"]
fn a_model_whose_action_step_exceeds_the_accepted_target_lead_is_refused() {
    let assets = policy_assets();
    let error = Preview::new(&config_accepting_target_lead(&assets, 0.04))
        .err()
        .unwrap();
    assert!(
        error.to_string().contains("max_target_lead_radians"),
        "{error:#}"
    );
}

#[test]
#[ignore = "requires a private SO-101 policy checkpoint and ORT_DYLIB_PATH"]
fn a_pose_beyond_reach_ends_once_the_arm_stops_closing_on_it() {
    let assets = policy_assets();
    let mut preview = Preview::new(&config(&assets)).unwrap().unwrap();
    let ended = preview
        .reach(&request(goal_near_start([1.5, 0.0, 0.0])))
        .unwrap();
    assert_eq!(ended.outcome, PreviewOutcome::TimedOut);
    assert!(ended.reason.is_some());
    // WHY: Once the arm rests short of the pose, the rest of the 8 s deadline would end at
    // the same joints, and a drag waits on every rehearsal.
    assert!(ended.simulated_seconds < 4.0, "{ended:?}");
}

#[test]
#[ignore = "requires a private SO-101 policy checkpoint and ORT_DYLIB_PATH"]
fn a_wrong_joint_count_is_refused() {
    let assets = policy_assets();
    let mut preview = Preview::new(&config(&assets)).unwrap().unwrap();
    let mut asked = request(goal_near_start([0.0, 0.0, 0.0]));
    asked.joints.pop();
    assert!(preview.reach(&asked).is_err());
}

#[test]
#[ignore = "requires a private SO-101 policy checkpoint and ORT_DYLIB_PATH"]
fn a_rehearsal_over_the_link_leaves_the_arm_and_its_records_alone() {
    let assets = policy_assets();
    let link = Link::new(&config(&assets), control()).unwrap();
    let handle = link.handle();
    let mut client = LinkClient::connect(&handle);
    let goal = goal_near_start([0.06, 0.0, 0.0]);
    let outcome = handle
        .project(
            goal.target.translation.vector.into(),
            Some(hebbian_contracts::PoseOrientation::Quaternion {
                quaternion: goal.target.rotation.coords.into(),
            }),
            &START,
            Some(0.0),
        )
        .unwrap()
        .outcome;
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

    client.snapshot();
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
    let mut without_policy = config(Path::new("unused-policy-directory")).into_input();
    without_policy.policy = None;
    let without_policy = ControllerConfig::try_from(without_policy).unwrap();
    assert!(Preview::new(&without_policy).unwrap().is_none());
}

#[test]
#[ignore = "requires a private SO-101 policy checkpoint and ORT_DYLIB_PATH"]
fn a_live_policy_reach_uses_direct_bounded_actions_and_completes_in_simulation() {
    use hebbian_rua_core::control_loop::run;
    use hebbian_rua_core::plant::NoPhysicsStandIn;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    let assets = policy_assets();
    let config = config(&assets);
    let link = Link::new(&config, control()).unwrap();
    let handle = link.handle();
    let stop = Arc::new(AtomicBool::new(false));
    let shutdown = Arc::clone(&stop);
    let loop_thread = std::thread::spawn(move || {
        let mut plant = NoPhysicsStandIn::at(START.to_vec(), 0.05).with_gripper(0.0);
        let report = run(
            &mut plant,
            &link,
            &config,
            Some(Duration::from_secs(5)),
            &shutdown,
        );
        link.close();
        report
    });
    let mut client = LinkClient::connect(&handle);
    client.command(hebbian_contracts::Command::Enable);
    client
        .read_until(|event| matches!(event,Event::State(sample) if sample.mode==ArmMode::Holding));
    let goal = goal_near_start([0.06, 0.0, 0.0]);
    client.command(hebbian_contracts::Command::ReachPose {
        label: "policy_move".into(),
        position: goal.target.translation.vector.into(),
        orientation: hebbian_contracts::PoseOrientation::Quaternion {
            quaternion: goal.target.rotation.coords.into(),
        },
    });
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

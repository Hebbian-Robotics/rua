//! Tests of the policy reach check and the servo-setpoint guards.

use nalgebra::Isometry3;

use super::guard_command;
use crate::arm::ReachGoal;
use crate::config::ExecutorConfig;
use crate::plant::Feedback;
use crate::policy::{ActiveReach, PolicyRuntime, ReachStatus};

fn config() -> ExecutorConfig {
    ExecutorConfig::parse(crate::config::TEST_CONFIG).unwrap()
}
fn feedback(positions: Vec<f64>) -> Feedback {
    Feedback {
        velocities: vec![0.0; positions.len()],
        efforts: vec![0.0; positions.len()],
        temperatures_celsius: vec![25.0; positions.len()],
        voltages_volts: Vec::new(),
        environment_confirmations: Vec::new(),
        positions,
        gripper_angle: None,
        fault: None,
    }
}
fn policy_settings() -> crate::config::PolicyConfig {
    crate::config::PolicyConfig {
        onnx: "unused.onnx".into(),
        tool_frame_fixture: "unused.json".into(),
        checkpoint_sha256: "unused".into(),
        tool_link: "tool".into(),
        reach_deadline_seconds: 8.0,
        settle_dwell_seconds: 0.1,
        settled_velocity_radians_per_second: 0.15,
        preview_lag_seconds: 0.05,
        position_tolerance_meters: 0.02,
        orientation_tolerance_radians: 0.1,
    }
}
#[test]
fn a_policy_must_remain_still_for_the_whole_settle_dwell() {
    let goal = ReachGoal {
        position: [0.0; 3],
        quaternion: [0.0, 0.0, 0.0, 1.0],
        position_tolerance_meters: 0.02,
        orientation_tolerance_radians: 0.1,
    };
    let mut reach = ActiveReach::new(goal, &policy_settings());
    let mut feedback = feedback(vec![0.0; 2]);
    feedback.velocities = vec![0.3; 2];
    for _ in 0..10 {
        assert_eq!(
            reach.assess_pose(&Isometry3::identity(), &feedback, 0.02),
            ReachStatus::Running
        );
    }
    feedback.velocities = vec![0.0; 2];
    for _ in 0..4 {
        assert_eq!(
            reach.assess_pose(&Isometry3::identity(), &feedback, 0.02),
            ReachStatus::Running
        );
    }
    assert_eq!(
        reach.assess_pose(&Isometry3::identity(), &feedback, 0.02),
        ReachStatus::Settled
    );
}
#[test]
fn policy_timing_and_action_ceilings_are_checked_before_loading_a_network() {
    let mut config = config().into_input();
    config.joint_count = 5;
    config.lower_limits = vec![-2.0; 5];
    config.upper_limits = vec![2.0; 5];
    config.joint_effort_limits = vec![10.0; 5];
    config.policy = Some(policy_settings());
    config.frequency_hertz = 60.0;
    assert!(
        PolicyRuntime::new(&ExecutorConfig::try_from(config.clone()).unwrap(), None)
            .err()
            .unwrap()
            .to_string()
            .contains("50 Hz")
    );
    config.frequency_hertz = 50.0;
    config.max_joint_speed = 0.6;
    config.default_joint_speed = 0.5;
    assert!(
        PolicyRuntime::new(&ExecutorConfig::try_from(config).unwrap(), None)
            .err()
            .unwrap()
            .to_string()
            .contains("action step")
    );
}
#[test]
fn a_setpoint_is_clamped_into_the_limits_and_only_recovers_inward_from_outside() {
    let config = config();
    // A hold at a measured pose that overshot the limit is held on the limit.
    let mut overshoot = [2.0001, -2.0001];
    super::clamp_setpoint(&config, &mut overshoot, &[2.0, -2.0]);
    assert_eq!(overshoot, [2.0, -2.0]);
    // From a start outside, it may stay or move inward, never farther out.
    let previous = [-2.02, 0.0];
    for (asked, sent) in [(-2.03, -2.02), (-2.02, -2.02), (-2.015, -2.015)] {
        let mut command = [asked, 0.0];
        super::clamp_setpoint(&config, &mut command, &previous);
        assert_eq!(command[0], sent);
    }
    let feedback = feedback(vec![-2.01, 0.0]);
    let mut not_finite = [f64::NAN, 0.0];
    super::clamp_setpoint(&config, &mut not_finite, &previous);
    assert!(guard_command(&config, None, None, &not_finite, &previous, &feedback).is_err());
}

#[test]
fn a_record_reports_the_tool_in_the_world_frame_targets_are_planned_in() {
    let urdf = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("robots/so101/robot.urdf");
    let mut text: serde_json::Value = serde_json::from_str(crate::config::TEST_CONFIG).unwrap();
    text["urdf"] = serde_json::json!(urdf);
    text["driven_joints"] = serde_json::json!([
        "shoulder_pan",
        "shoulder_lift",
        "elbow_flex",
        "wrist_flex",
        "wrist_roll"
    ]);
    text["tool_link"] = serde_json::json!("gripper_frame_link");
    text["base_pose"] =
        serde_json::json!({"position_meters": [0.3, -0.2, 0.05], "yaw_radians": 0.8});
    text["joint_count"] = serde_json::json!(5);
    text["lower_limits"] = serde_json::json!(vec![-1.5; 5]);
    text["upper_limits"] = serde_json::json!(vec![1.5; 5]);
    text["joint_effort_limits"] = serde_json::json!(vec![10.0; 5]);
    let config = ExecutorConfig::parse(&text.to_string()).unwrap();
    let robot = super::load_shared_robot(&config).unwrap();
    let tool = super::tool_pose::ToolPose::new(&config, robot.as_ref())
        .unwrap()
        .unwrap();
    let planner = crate::planning::Planner::for_arm(&config).unwrap().unwrap();
    let joints = vec![0.3, -0.6, 0.9, 0.2, 0.4];
    let (recorded_position, _) = tool.pose(&feedback(joints.clone())).unwrap();
    let planned_position = planner.describe(&joints).unwrap().position;
    let gap: f64 = recorded_position
        .iter()
        .zip(planned_position)
        .map(|(a, b)| (a - b).powi(2))
        .sum::<f64>()
        .sqrt();
    assert!(gap < 1e-12, "the record and the plan disagree by {gap} m");
}

#[test]
fn combined_motion_and_opening_keep_each_motor_cycle_within_the_deadline() {
    use crate::arm::Now;
    use crate::plant::{Plant, SimulatedPlant};
    use hebbian_contracts::{Command, CommandOutcome, PoseOrientation, Waypoint};
    use std::time::{Duration, Instant};

    let config = combined_motion_config();
    let initial = vec![0.0, -0.5, 0.5, 0.5, 0.0];
    let mut destination = initial.clone();
    destination[0] = 0.15;
    let planner = crate::planning::Planner::for_arm(&config).unwrap().unwrap();
    let pose = planner.describe(&destination).unwrap();
    let motion = planner
        .plan(
            &initial,
            &[Waypoint::Pose {
                position: pose.position,
                orientation: PoseOrientation::Quaternion {
                    quaternion: pose.quaternion,
                },
            }],
            Some(1.0),
        )
        .unwrap();
    let mut plant = SimulatedPlant::at(initial.clone(), 0.0).with_gripper(0.0);
    let mut runner = super::runner_for(
        "arm",
        &mut plant,
        None,
        &config,
        std::path::Path::new(&config.trace_directory),
    )
    .ok()
    .unwrap();
    runner.state.power_up(&initial, Some(0.0));
    runner.state.core.enabled(&initial, Now::current());
    let movement = runner
        .state
        .core
        .submit(
            Command::Move {
                label: "combined".into(),
                plan: motion.plan,
                joint_speed: 1.0,
                path_limited: None,
                pose_limited: None,
            },
            Now::current(),
        )
        .unwrap();
    let opening = runner
        .state
        .core
        .submit(
            Command::SetGripper {
                angle: 1.5,
                joint_speed: Some(1.0),
            },
            Now::current(),
        )
        .unwrap();
    let mut longest_cycle = Duration::ZERO;
    for _ in 0..250 {
        let began = Instant::now();
        let observation = runner.plant.read_feedback().unwrap();
        runner.start_next(Now::current());
        runner
            .drive_gripper(&observation, 0.02, Now::current())
            .unwrap();
        runner
            .state
            .command(0.02, &observation, false, None, Now::current())
            .unwrap();
        runner
            .plant
            .command(&runner.state.next_command, 0.02)
            .unwrap();
        runner
            .state
            .previous_command
            .clone_from(&runner.state.next_command);
        longest_cycle = longest_cycle.max(began.elapsed());
    }
    eprintln!("longest combined motor cycle: {longest_cycle:?}");
    assert!(longest_cycle.as_secs_f64() < config.loop_deadline_seconds);
    for record in [movement, opening] {
        assert_eq!(
            runner
                .state
                .core
                .record(&record.command_id)
                .unwrap()
                .outcome(),
            Some(CommandOutcome::Completed)
        );
    }
    assert!((runner.plant.read_feedback().unwrap().gripper_angle.unwrap() - 1.5).abs() < 0.01);
}

fn combined_motion_config() -> ExecutorConfig {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    let mut profile =
        crate::profile::load_robot_profile(&root.join("profiles"), "so101-sim").unwrap();
    profile.urdf = root.join("robots/so101/robot.urdf");
    let mut config = crate::profile::executor_config_input(
        &profile,
        "follower",
        crate::profile::JointLimitSource::Urdf,
        root,
    )
    .unwrap();
    config.trace_directory = std::env::temp_dir()
        .join(format!("combined-motion-{}", std::process::id()))
        .to_string_lossy()
        .into_owned();
    ExecutorConfig::try_from(config).unwrap()
}

#[test]
fn admission_yields_at_its_budget_and_resumes_with_the_next_arm() {
    use crate::handoff::arm_channel;
    use crate::plant::SimulatedPlant;
    use hebbian_contracts::Command;
    use std::time::Duration;
    let mut config = config().into_input();
    config.trace_directory = std::env::temp_dir()
        .join("admission-budget-traces")
        .to_string_lossy()
        .into_owned();
    let config = ExecutorConfig::try_from(config).unwrap();
    let mut left_plant = SimulatedPlant::at(vec![0.0; 2], 0.0);
    let mut right_plant = SimulatedPlant::at(vec![0.0; 2], 0.0);
    let mut runners = vec![
        super::runner_for(
            "left",
            &mut left_plant,
            None,
            &config,
            std::path::Path::new(&config.trace_directory),
        )
        .ok()
        .unwrap(),
        super::runner_for(
            "right",
            &mut right_plant,
            None,
            &config,
            std::path::Path::new(&config.trace_directory),
        )
        .ok()
        .unwrap(),
    ];
    let (left_handle, left_inbox) = arm_channel();
    let (right_handle, right_inbox) = arm_channel();
    runners[0].inbox = Some(left_inbox);
    runners[1].inbox = Some(right_inbox);
    let left_first = left_handle.submit_unawaited(Command::Enable, None).unwrap();
    let left_second = left_handle.submit_unawaited(Command::Enable, None).unwrap();
    let right_first = right_handle
        .submit_unawaited(Command::Enable, None)
        .unwrap();
    let mut next_request_arm = 0;
    super::serve_requests(&mut runners, Duration::ZERO, &mut next_request_arm);
    assert!(left_first.try_recv().unwrap().is_ok());
    assert!(left_second.try_recv().is_err());
    assert!(right_first.try_recv().is_err());
    super::serve_requests(&mut runners, Duration::ZERO, &mut next_request_arm);
    assert!(right_first.try_recv().unwrap().is_ok());
    assert!(left_second.try_recv().is_err());
    super::serve_requests(&mut runners, Duration::from_secs(1), &mut next_request_arm);
    assert_eq!(
        left_second.try_recv().unwrap().unwrap_err().reason,
        hebbian_contracts::RejectionReason::ArmBusy
    );
    for runner in runners {
        let _ = runner.trace.finish();
    }
}

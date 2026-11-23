use std::fmt::Write;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::orientation::{grasp_rotation, pose_orientation_quaternion, xyzw};
use super::{PlannedMotion, Planner, ProjectionOutcome, Rejection, Waypoint};
use crate::config::ControllerConfig;
use crate::preview::{PreviewOutcome, ReachPreview};
use hebbian_contracts::{
    Approach, JawClosingAxis, NamedApproach, PoseOrientation, RejectionReason,
};

fn config(urdf: &Path, joints: &[&str], tool_link: &str, settings: &Value) -> ControllerConfig {
    let mut text: Value = serde_json::from_str(crate::config::TEST_CONFIG).unwrap();
    text["urdf"] = json!(urdf);
    text["driven_joints"] = json!(joints);
    text["tool_link"] = json!(tool_link);
    for (key, value) in settings.as_object().unwrap() {
        text[key] = value.clone();
    }
    ControllerConfig::parse(&text.to_string()).unwrap()
}

fn seven_joint_planner(name: &str) -> Planner {
    let links: Vec<String> = (0..=7).map(|index| format!("link{index}")).collect();
    let mut urdf = String::from(r#"<robot name="chain">"#);
    for link in &links {
        write!(urdf, r#"<link name="{link}"/>"#).unwrap();
    }
    for index in 0..7 {
        write!(
            urdf,
            r#"<joint name="joint{}" type="revolute"><parent link="{}"/><child link="{}"/>
               <origin xyz="1 0 0"/><axis xyz="0 0 1"/>
               <limit lower="-1" upper="1" effort="1" velocity="1"/></joint>"#,
            index + 1,
            links[index],
            links[index + 1]
        )
        .unwrap();
    }
    urdf.push_str("</robot>");
    let path = std::env::temp_dir().join(format!("{name}-{}.urdf", std::process::id()));
    std::fs::write(&path, urdf).unwrap();
    let joints: Vec<String> = (1..=7).map(|index| format!("joint{index}")).collect();
    let joint_names: Vec<&str> = joints.iter().map(String::as_str).collect();
    let planner = Planner::for_arm(&config(
        &path,
        &joint_names,
        "link7",
        &json!({
            "joint_count": 7,
            "lower_limits": vec![-1.0; 7],
            "upper_limits": vec![1.0; 7],
            "joint_effort_limits": vec![10.0; 7],
        }),
    ))
    .unwrap()
    .unwrap();
    let _ = std::fs::remove_file(path);
    planner
}

fn joints(values: &[f64]) -> Waypoint {
    let mut all = values.to_vec();
    all.resize(7, 0.0);
    Waypoint::Joints { joints: all }
}

fn planned(planner: &Planner, waypoints: &[Waypoint]) -> PlannedMotion {
    planner
        .plan(
            &[0.0; 7],
            waypoints,
            Some(hebbian_contracts::JointSpeed::new(0.5).unwrap()),
            None,
        )
        .unwrap()
}

fn last_corner(motion: &PlannedMotion) -> &[f64] {
    motion.plan.segments.last().unwrap().corners.last().unwrap()
}

fn close(found: &[f64], expected: &[f64]) -> bool {
    found.len() == expected.len()
        && found
            .iter()
            .zip(expected)
            .all(|(a, b)| (a - b).abs() < 1e-9)
}

fn refused(planner: &Planner, start: &[f64], waypoint: Waypoint) -> Rejection {
    planner.plan(start, &[waypoint], None, None).unwrap_err()
}

#[test]
fn a_target_on_each_bound_is_kept_and_one_past_it_is_refused() {
    let planner = seven_joint_planner("limits");
    for joint in 0..7 {
        for bound in [-1.0, 1.0] {
            let mut on_bound = [0.0; 7];
            on_bound[joint] = bound;
            let motion = planned(
                &planner,
                &[Waypoint::Joints {
                    joints: on_bound.to_vec(),
                }],
            );
            assert_eq!(last_corner(&motion)[joint], bound);
            // WHY: From the bound too, as a plan starts where the last one ended.
            planner
                .plan(&on_bound, &[joints(&[0.0])], None, None)
                .unwrap();
        }
    }
    for past in [1.0 + 1e-12, 5.0] {
        let refusal = refused(&planner, &[0.0; 7], joints(&[past, -past]));
        assert_eq!(refusal.reason, RejectionReason::InvalidTarget);
        assert!(refusal.detail.contains("outside"));
    }
    let short = refused(
        &planner,
        &[0.0; 7],
        Waypoint::Joints {
            joints: vec![0.0; 6],
        },
    );
    assert_eq!(short.reason, RejectionReason::InvalidTarget);
    assert!(short.detail.contains("Expected 7"), "{}", short.detail);
}

#[test]
fn a_base_pose_moves_and_turns_the_whole_arm_in_the_world() {
    // CONSTRAINT: One z joint at the base and a tool a metre along x from it.
    let path = std::env::temp_dir().join(format!("base-pose-{}.urdf", std::process::id()));
    std::fs::write(
        &path,
        r#"<robot name="placed"><link name="world"/><link name="arm"/><link name="tool"/>
           <joint name="turn" type="revolute"><parent link="world"/><child link="arm"/>
             <axis xyz="0 0 1"/><limit lower="-1" upper="1" effort="1" velocity="1"/></joint>
           <joint name="tip" type="fixed"><parent link="arm"/><child link="tool"/>
             <origin xyz="1 0 0"/></joint></robot>"#,
    )
    .unwrap();
    let planner = Planner::for_arm(&config(
        &path,
        &["turn"],
        "tool",
        &json!({
            "joint_count": 1, "lower_limits": [-1.0], "upper_limits": [1.0],
            "joint_effort_limits": [10.0],
            "base_pose": {"position_meters": [0.5, -0.2, 0.1],
                          "yaw_radians": std::f64::consts::FRAC_PI_2},
        }),
    ))
    .unwrap()
    .unwrap();
    let _ = std::fs::remove_file(path);
    // INVARIANT: Turned a quarter about z: the tool's metre along x now points along +y.
    let at_zero = planner.describe(&[0.0]).unwrap().position;
    let turned = planner.describe(&[0.3]).unwrap().position;
    assert!(close(&at_zero, &[0.5, 0.8, 0.1]), "{at_zero:?}");
    assert!(close(
        &turned,
        &[0.5 - 0.3_f64.sin(), -0.2 + 0.3_f64.cos(), 0.1]
    ));
}

const SO101_JOINTS: [&str; 6] = [
    "shoulder_pan",
    "shoulder_lift",
    "elbow_flex",
    "wrist_flex",
    "wrist_roll",
    "gripper",
];
const SO101_LOWER: [f64; 5] = [-1.91986, -1.74533, -1.69, -1.65806, -2.74385];
const SO101_UPPER: [f64; 5] = [1.91986, 1.74533, 1.69, 1.65806, 2.84121];
const OVER_THE_TABLE: [f64; 5] = [0.0, -0.5, 0.8, 0.6, 0.0];

fn bundled_so101() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("robots/so101/robot.urdf")
}

fn so101_planner(settings: &Value) -> Planner {
    let mut all = json!({
        "joint_count": 5,
        "lower_limits": SO101_LOWER,
        "upper_limits": SO101_UPPER,
        "joint_effort_limits": vec![10.0; 5],
    });
    for (key, value) in settings.as_object().unwrap() {
        all[key] = value.clone();
    }
    Planner::for_arm(&config(
        &bundled_so101(),
        &SO101_JOINTS,
        "gripper_frame_link",
        &all,
    ))
    .unwrap()
    .unwrap()
}

#[test]
fn turning_the_so101_wrist_roll_turns_the_jaw_yaw_by_as_much_and_nothing_else() {
    // INVARIANT: The jaw yaw ranges rest on this: the wrist roll turns about minus the
    // approach, positive roll toward positive yaw.
    let planner = so101_planner(&json!({}));
    let mut rolled = OVER_THE_TABLE;
    rolled[4] = 0.3;
    let before = planner.describe(&OVER_THE_TABLE).unwrap();
    let after = planner.describe(&rolled).unwrap();
    // CONSTRAINT: The tool frame sits a few millimetres off the roll axis.
    let moved: f64 = before
        .position
        .iter()
        .zip(after.position)
        .map(|(a, b)| (a - b).powi(2))
        .sum::<f64>()
        .sqrt();
    assert!(moved < 0.005, "{moved}");
    let approach_turn: f64 = before
        .orientation
        .approach
        .iter()
        .zip(after.orientation.approach)
        .map(|(a, b)| (a - b).abs())
        .sum();
    assert!(approach_turn < 1e-3, "{approach_turn}");
    let yaw_turn =
        (after.orientation.jaw_yaw_degrees - before.orientation.jaw_yaw_degrees).rem_euclid(360.0);
    assert!((yaw_turn - 0.3_f64.to_degrees()).abs() < 0.2, "{yaw_turn}");
}

#[test]
fn the_so101_zero_pose_points_the_jaws_level_and_closing_vertically() {
    let home = planner_home();
    assert!((home.degrees_from_down - 90.0).abs() < 0.2);
    assert!((home.jaw_yaw_degrees - 90.0).abs() < 3.0);
}

fn planner_home() -> hebbian_contracts::OrientationSummary {
    so101_planner(&json!({}))
        .describe(&[0.0; 5])
        .unwrap()
        .orientation
}

fn pose_at(planner: &Planner, joints: &[f64]) -> ([f64; 3], [f64; 4]) {
    let description = planner.describe(joints).unwrap();
    (description.position, description.quaternion)
}

#[test]
fn the_ghost_of_a_reachable_pose_lists_the_jaw_yaws_the_wrist_can_take() {
    let planner = so101_planner(&json!({}));
    let (position, quaternion) = pose_at(&planner, &OVER_THE_TABLE);
    let projection = planner.project(
        &OVER_THE_TABLE,
        position,
        Some(PoseOrientation::Quaternion { quaternion }),
        None,
    );
    assert!(matches!(
        projection.outcome,
        ProjectionOutcome::Projected {
            within_tolerance: true,
            ..
        }
    ));
    let ranges = &projection.jaw_yaw_ranges;
    assert!(ranges.iter().any(|span| span.reachable));
    // CONSTRAINT: The wrist roll's range is under a full turn, so some yaws are out of reach.
    assert!(ranges.iter().any(|span| !span.reachable));
    for span in ranges {
        assert!(span.from_degrees < span.to_degrees);
        assert_eq!(span.reason.is_none(), span.reachable);
    }
    assert!(ranges[0].from_degrees.abs() < 1e-9);
    assert!((ranges.last().unwrap().to_degrees - 360.0).abs() < 1e-9);
    let requested = projection.requested.unwrap();
    let again = grasp_rotation(
        Approach::Direction(requested.orientation.approach),
        requested.orientation.jaw_yaw_degrees,
        hebbian_contracts::JawClosingAxis::X,
    )
    .unwrap();
    let alignment: f64 = xyzw(&again)
        .iter()
        .zip(quaternion)
        .map(|(a, b)| a * b)
        .sum();
    assert!(1.0 - alignment.abs() < 1e-9);
}

#[test]
fn a_measured_pose_just_past_a_limit_is_previewed_and_one_far_past_is_not() {
    let planner = so101_planner(&json!({"joint_limit_recovery_tolerance_radians": 0.035}));
    let mut at_limit = [0.0; 5];
    at_limit[0] = SO101_UPPER[0];
    let (position, quaternion) = pose_at(&planner, &at_limit);
    let orientation = Some(PoseOrientation::Quaternion { quaternion });
    let mut measured = at_limit;
    measured[0] += 0.01;
    let near = planner.project(&measured, position, orientation, None);
    let ProjectionOutcome::Projected { pose, .. } = near.outcome else {
        panic!("not previewed: {near:?}");
    };
    assert!(pose.joints[0] <= SO101_UPPER[0]);
    measured[0] = SO101_UPPER[0] + 0.1;
    let far = planner.project(&measured, position, orientation, None);
    assert!(matches!(far.outcome, ProjectionOutcome::Unavailable { .. }));
}

#[test]
fn leaving_the_orientation_free_reaches_a_position_a_fixed_one_cannot() {
    let planner = so101_planner(&json!({}));
    let position = [0.2, 0.0, 0.15];
    let down = Some(PoseOrientation::Grasp {
        approach: Approach::Named(NamedApproach::Down),
        jaw_yaw_degrees: 180.0,
    });
    let fixed = planner.project(&OVER_THE_TABLE, position, down, None);
    assert!(!matches!(
        fixed.outcome,
        ProjectionOutcome::Projected {
            within_tolerance: true,
            ..
        }
    ));
    let free = planner.project(&OVER_THE_TABLE, position, None, None);
    let ProjectionOutcome::Projected {
        pose,
        within_tolerance: true,
        ..
    } = free.outcome
    else {
        panic!("not reached: {free:?}");
    };
    assert!(pose.position_error_meters < 0.005);
    assert!(pose.orientation_error_radians.is_none());
    assert!(free.requested.is_none());
    assert_ne!(free.jaw_yaw_ranges.len(), 0);
    let chosen = grasp_rotation(
        Approach::Direction(pose.orientation.approach),
        pose.orientation.jaw_yaw_degrees,
        hebbian_contracts::JawClosingAxis::X,
    )
    .unwrap();
    let alignment: f64 = xyzw(&chosen)
        .iter()
        .zip(pose.quaternion)
        .map(|(a, b)| a * b)
        .sum();
    assert!(1.0 - alignment.abs() < 1e-9);
}

#[test]
fn a_yaw_the_wrist_cannot_take_runs_to_the_ghosts_pose_and_advises_one_that_works() {
    let planner = so101_planner(&json!({}));
    let position = [0.2, 0.0, 0.05];
    let orientation = PoseOrientation::Grasp {
        approach: Approach::Named(NamedApproach::Down),
        jaw_yaw_degrees: 0.0,
    };
    let motion = planner
        .plan(
            &OVER_THE_TABLE,
            &[Waypoint::Pose {
                position,
                orientation,
            }],
            None,
            None,
        )
        .unwrap();
    let limited = motion.pose_limited.clone().unwrap();
    assert!(limited.position_shortfall_meters > 0.0);
    assert!(
        limited.advice.starts_with(&format!(
            "The solver did not reach the target: the closest pose it found is {:.1} mm",
            limited.position_shortfall_meters * 1000.0
        )),
        "{}",
        limited.advice
    );
    let ProjectionOutcome::Projected {
        pose,
        within_tolerance: false,
        ..
    } = planner
        .project(&OVER_THE_TABLE, position, Some(orientation), None)
        .outcome
    else {
        panic!("the ghost stops short too");
    };
    assert_eq!(last_corner(&motion), pose.joints.as_slice());
    assert!(
        limited
            .advice
            .contains("jaw_yaw_degrees 0 is not reachable"),
        "{}",
        limited.advice
    );
    let suggestion: Value =
        serde_json::from_str(&limited.advice[limited.advice.find("{\"approach\"").unwrap()..])
            .unwrap();
    assert_eq!(suggestion["approach"], json!([0.0, 0.0, -1.0]));
    let yaw = suggestion["jaw_yaw_degrees"].as_f64().unwrap();
    assert!(yaw.abs() > 5.0);
    let exact = planner
        .plan(
            &OVER_THE_TABLE,
            &[Waypoint::Pose {
                position,
                orientation: PoseOrientation::Grasp {
                    approach: Approach::Direction([0.0, 0.0, -1.0]),
                    jaw_yaw_degrees: yaw,
                },
            }],
            None,
            None,
        )
        .unwrap();
    assert_eq!(exact.pose_limited, None);
}

fn rehearsal(outcome: PreviewOutcome, reason: Option<&str>) -> ReachPreview {
    ReachPreview {
        outcome,
        joints: OVER_THE_TABLE.to_vec(),
        position: [0.2, 0.0, 0.1],
        quaternion: [0.0, 0.0, 0.0, 1.0],
        position_error_meters: 0.004,
        orientation_error_radians: 0.05,
        steps: 80,
        simulated_seconds: 1.6,
        reason: reason.map(str::to_owned),
    }
}

#[test]
fn the_policy_ghost_is_where_the_rehearsal_ended() {
    let planner = so101_planner(&json!({}));
    let (position, quaternion) = pose_at(&planner, &OVER_THE_TABLE);
    let target = Some(PoseOrientation::Quaternion { quaternion });
    let settled = planner.project_by_policy(&OVER_THE_TABLE, position, target, |_, _| {
        Ok(rehearsal(PreviewOutcome::Settled, None))
    });
    let ProjectionOutcome::Projected {
        pose,
        within_tolerance: true,
        ..
    } = settled.outcome
    else {
        panic!("not reachable: {settled:?}");
    };
    assert_eq!(pose.position, [0.2, 0.0, 0.1]);
    let unsettled = planner.project_by_policy(&OVER_THE_TABLE, position, target, |_, _| {
        Ok(rehearsal(PreviewOutcome::TimedOut, Some("Did not settle")))
    });
    let ProjectionOutcome::Projected {
        reason: Some(reason),
        within_tolerance: false,
        ..
    } = unsettled.outcome
    else {
        panic!("not constrained: {unsettled:?}");
    };
    assert!(reason.contains("Did not settle") && reason.contains("mm short of the goal"));
    let requested = unsettled.requested.unwrap().quaternion;
    let alignment: f64 = requested.iter().zip(quaternion).map(|(a, b)| a * b).sum();
    assert!(1.0 - alignment.abs() < 1e-12);
    for answer in [
        Ok(rehearsal(PreviewOutcome::Blocked, Some("Below the table"))),
        Err("Refused: no policy".to_owned()),
    ] {
        let ghost =
            planner.project_by_policy(&OVER_THE_TABLE, position, target, |_, _| answer.clone());
        assert!(matches!(
            ghost.outcome,
            ProjectionOutcome::Unavailable { .. }
        ));
    }
}

#[test]
fn the_policy_is_given_the_goal_a_move_would_send() {
    let planner = so101_planner(&json!({}));
    let mut asked = Vec::new();
    let down = PoseOrientation::Grasp {
        approach: Approach::Named(NamedApproach::Down),
        jaw_yaw_degrees: 90.0,
    };
    let ghost = planner.project_by_policy(&OVER_THE_TABLE, [0.2, 0.0, 0.1], Some(down), |p, q| {
        asked.push((p, q));
        Ok(rehearsal(PreviewOutcome::Settled, None))
    });
    let [(position, quaternion)] = asked.as_slice() else {
        panic!("rehearsed {} times", asked.len());
    };
    assert_eq!(*position, [0.2, 0.0, 0.1]);
    let wanted = pose_orientation_quaternion(down, JawClosingAxis::X).unwrap();
    assert!(close(quaternion, &wanted));
    let requested = ghost.requested.unwrap().orientation;
    assert!((requested.jaw_yaw_degrees - 90.0).abs() < 1e-9);
    assert_ne!(ghost.jaw_yaw_ranges.len(), 0);

    // WHY: Position alone: the goal takes the orientation position-only solving chose,
    // reported as requested so a move can send the same.
    let mut chosen = Vec::new();
    let free = planner.project_by_policy(&OVER_THE_TABLE, [0.2, 0.0, 0.15], None, |p, q| {
        chosen.push((p, q));
        Ok(rehearsal(PreviewOutcome::Settled, None))
    });
    let [(_, quaternion)] = chosen.as_slice() else {
        panic!("rehearsed {} times", chosen.len());
    };
    assert!(close(&free.requested.unwrap().quaternion, quaternion));
    assert_ne!(free.jaw_yaw_ranges.len(), 0);
}

const NOMINAL: [f64; 5] = [0.0, -1.0, 1.0, 0.5, 0.0];
const CLIPPED: [f64; 5] = [
    -0.591_467_913_2,
    0.038_627_551_3,
    1.322_287_804_1,
    0.913_803_100_9,
    -0.966_982_146_8,
];
// CONSTRAINT: The arm alone is clear here, but the open jaw swings into the base.
const JAW_ONLY_COLLISION: [f64; 5] = [-1.367, 1.5004, 0.343, 1.2259, 2.7595];

fn so101_self_collision() -> Planner {
    so101_planner(&json!({"self_collision": {"penetration_tolerance_meters": 0.01}}))
}

fn plan_to(planner: &Planner, start: &[f64], goal: &[f64]) -> Result<PlannedMotion, Rejection> {
    planner.plan(
        start,
        &[Waypoint::Joints {
            joints: goal.to_vec(),
        }],
        None,
        None,
    )
}

fn path_refusal(planner: &Planner, start: &[f64], goal: &[f64]) -> Option<String> {
    match plan_to(planner, start, goal) {
        Ok(motion) => motion.path_limited.map(|limited| limited.reason),
        Err(refused) => Some(refused.detail),
    }
}

#[test]
fn nominal_and_folded_so101_poses_are_accepted_and_a_clipping_path_is_not() {
    let planner = so101_self_collision();
    for joints in [NOMINAL, [0.0, -1.74, 1.65, 1.28, -1.5]] {
        assert_eq!(path_refusal(&planner, &joints, &joints), None);
    }
    let reason = path_refusal(&planner, &NOMINAL, &CLIPPED).unwrap();
    assert!(reason.contains("Self-collision"), "{reason}");
}

#[test]
fn a_move_into_a_collision_runs_to_the_last_clear_sample_and_says_why() {
    let planner = so101_self_collision();
    let motion = plan_to(&planner, &NOMINAL, &CLIPPED).unwrap();
    let limited = motion.path_limited.as_ref().unwrap();
    assert!(
        limited.reason.contains("Self-collision"),
        "{}",
        limited.reason
    );
    let segment = motion.plan.segments.last().unwrap();
    let stop = segment.corners.last().unwrap();
    assert!(!close(stop, &CLIPPED));
    assert_eq!(segment.destination_position, None);
    assert_eq!(path_refusal(&planner, &NOMINAL, stop), None);
    // WHY: The next sample on toward the target is the one the guard stops.
    let toward: Vec<f64> = stop
        .iter()
        .zip(&CLIPPED)
        .map(|(at, goal)| at + (goal - at) * 0.5)
        .collect();
    assert!(path_refusal(&planner, stop, &toward).is_some());
}

#[test]
fn clear_endpoints_do_not_certify_a_path_that_clips_between_them() {
    let planner = so101_self_collision();
    let start = [
        0.891_252_430_2,
        -0.863_353_674,
        1.178_565_928_9,
        1.468_236_241_9,
        -1.329_266_097_7,
    ];
    let goal = [
        1.082_822_760_5,
        1.483_126_850_8,
        0.288_876_925,
        1.483_049_233_3,
        1.463_302_554_7,
    ];
    assert_eq!(path_refusal(&planner, &start, &start), None);
    assert_eq!(path_refusal(&planner, &goal, &goal), None);
    assert!(path_refusal(&planner, &start, &goal).is_some());
}

#[test]
fn the_ghost_checks_the_jaw_at_its_measured_opening() {
    let planner = so101_self_collision();
    let (position, quaternion) = pose_at(&planner, &JAW_ONLY_COLLISION);
    let orientation = Some(PoseOrientation::Quaternion { quaternion });
    // WHY: From the target itself the solve stays put, so only the jaw decides.
    let closed = planner.project(&JAW_ONLY_COLLISION, position, orientation, Some(0.0));
    assert!(
        matches!(
            closed.outcome,
            ProjectionOutcome::Projected {
                within_tolerance: true,
                ..
            }
        ),
        "{closed:?}"
    );
    let open = planner.project(&JAW_ONLY_COLLISION, position, orientation, Some(1.659));
    let ProjectionOutcome::Unavailable { reason } = open.outcome else {
        panic!("the open jaw was not checked: {open:?}");
    };
    assert!(reason.contains("moving_jaw"), "{reason}");
    let waypoint = Waypoint::Joints {
        joints: JAW_ONLY_COLLISION.to_vec(),
    };
    assert!(
        planner
            .plan(
                &JAW_ONLY_COLLISION,
                std::slice::from_ref(&waypoint),
                None,
                Some(0.0)
            )
            .is_ok()
    );
    let refused = planner
        .plan(&JAW_ONLY_COLLISION, &[waypoint], None, Some(1.659))
        .unwrap_err();
    assert!(refused.detail.contains("moving_jaw"), "{}", refused.detail);
}

#[test]
fn the_table_stops_a_descent_into_it_and_lets_the_arm_climb_out() {
    let planner = so101_planner(&json!({
        "surface_guard": {"surface_height_meters": 0.0, "leeway_meters": 0.01}
    }));
    let reaching_over_the_table = [0.0; 5];
    let plunging_into_it = [0.0, 1.5, 0.0, 0.0, 0.0];
    let reason = path_refusal(&planner, &reaching_over_the_table, &plunging_into_it).unwrap();
    assert!(reason.contains("table"), "{reason}");
    assert_eq!(
        path_refusal(&planner, &plunging_into_it, &reaching_over_the_table),
        None
    );
    let below = planner.describe(&plunging_into_it).unwrap().position;
    let ghost = planner.project(&reaching_over_the_table, below, None, None);
    let ProjectionOutcome::Projected {
        pose,
        reason: Some(reason),
        within_tolerance: false,
    } = ghost.outcome
    else {
        panic!("the table did not stop the ghost: {ghost:?}");
    };
    assert!(reason.contains("table"), "{reason}");
    assert!(pose.position_error_meters > 0.0);
}

#[test]
fn pose_move_refines_a_target_near_the_self_collision_boundary() {
    let planner = so101_self_collision();
    let start = [
        1.075_103_082_395_820_3,
        0.120_430_248_690_214_9,
        0.591_406_763_918_093_8,
        0.258_016_149_914_850_33,
        1.094_233_807_511_805_4,
    ];
    let clear_target = [
        0.186_255_197_729_281,
        -0.570_396_711_448_777_1,
        1.674_865_139_528_378_7,
        0.330_231_309_992_021_77,
        -1.832_793_433_316_737_3,
    ];
    assert_eq!(path_refusal(&planner, &start, &clear_target), None);
    let target_pose = planner.describe(&clear_target).unwrap();
    let orientation = PoseOrientation::Quaternion {
        quaternion: target_pose.quaternion,
    };
    let preview = planner.project(&start, target_pose.position, Some(orientation), None);
    assert!(
        matches!(
            preview.outcome,
            ProjectionOutcome::Projected {
                within_tolerance: true,
                ..
            }
        ),
        "{:?}",
        preview.outcome
    );
    let motion = planner
        .plan(
            &start,
            &[Waypoint::Pose {
                position: target_pose.position,
                orientation,
            }],
            None,
            None,
        )
        .unwrap();
    assert!(motion.pose_limited.is_none());
    assert!(motion.path_limited.is_none(), "{:?}", motion.path_limited);
}

mod reach_sweep;

#[test]
fn pose_move_finds_a_clear_posture_after_early_starts_stall() {
    let planner = so101_self_collision();
    let start = [
        -0.819_287_355,
        -0.391_482_722,
        0.694_807_825,
        1.184_636_766,
        0.931_994_259,
    ];
    let reference_joints = [
        0.191_764_166,
        -1.260_136_426,
        0.261_191_393,
        -1.150_543_162,
        -0.970_721_025,
    ];
    assert_eq!(path_refusal(&planner, &start, &reference_joints), None);
    let target = planner.describe(&reference_joints).unwrap();
    let motion = planner
        .plan(
            &start,
            &[Waypoint::Pose {
                position: target.position,
                orientation: PoseOrientation::Quaternion {
                    quaternion: target.quaternion,
                },
            }],
            None,
            Some(1.5),
        )
        .unwrap();
    assert!(motion.pose_limited.is_none(), "{:?}", motion.pose_limited);
    assert!(motion.path_limited.is_none(), "{:?}", motion.path_limited);
}

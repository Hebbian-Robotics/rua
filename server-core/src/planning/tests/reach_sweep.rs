use std::io::{LineWriter, Write};
use std::time::Instant;

use hebbian_contracts::IkFailure;
use serde::Serialize;
use serde_json::json;

use super::{Planner, PoseOrientation, Waypoint, so101_planner, so101_self_collision};
use crate::planning::PathCheck;

#[derive(Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
enum ReachOutcome {
    ReferenceBlocked,
    Refused {
        detail: String,
    },
    PoseLimited {
        reason: IkFailure,
    },
    PathLimited {
        reason: String,
    },
    Planned {
        position_error_meters: f64,
        orientation_error_radians: f64,
    },
}

#[derive(Serialize)]
struct ReachCase {
    profile: &'static str,
    seed: u64,
    index: usize,
    start: Vec<f64>,
    reference_joints: Vec<f64>,
    target_position: [f64; 3],
    target_quaternion: [f64; 4],
    gripper_angle: Option<f64>,
    plan_microseconds: Option<u128>,
    result: ReachOutcome,
}

struct JointSampler(u64);

impl JointSampler {
    fn sample(&mut self, planner: &Planner) -> Vec<f64> {
        planner
            .lower
            .iter()
            .zip(&planner.upper)
            .map(|(lower, upper)| {
                self.0 = self
                    .0
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1);
                let fraction =
                    f64::from(u32::try_from(self.0 >> 32).unwrap()) / f64::from(u32::MAX);
                lower + (upper - lower) * fraction
            })
            .collect()
    }
}

fn run_profile(
    output: &mut impl Write,
    profile: &'static str,
    planner: &Planner,
    seed: u64,
    case_count: usize,
    gripper_angle: Option<f64>,
) {
    let replay_profile = std::env::var("RUA_REACH_SWEEP_PROFILE").ok();
    if replay_profile
        .as_deref()
        .is_some_and(|selected| selected != profile)
    {
        return;
    }
    let first_case = std::env::var("RUA_REACH_SWEEP_START").map_or(0, |value| {
        value.parse::<usize>().expect("invalid starting case index")
    });
    let end_case = std::env::var("RUA_REACH_SWEEP_END").map_or(case_count, |value| {
        value.parse::<usize>().expect("invalid ending case index")
    });
    assert!(
        first_case < end_case && end_case <= case_count,
        "case range outside {profile}"
    );
    let mut sampler = JointSampler(seed);
    for index in 0..end_case {
        let start = sampler.sample(planner);
        let reference_joints = sampler.sample(planner);
        if index < first_case {
            continue;
        }
        let pose = planner.describe(&reference_joints).unwrap();
        let (plan_microseconds, result) = if planner
            .path_check(&start, &reference_joints, gripper_angle)
            .unwrap()
            == PathCheck::Clear
        {
            let target = Waypoint::Pose {
                position: pose.position,
                orientation: PoseOrientation::Quaternion {
                    quaternion: pose.quaternion,
                },
            };
            let started = Instant::now();
            let plan = planner.plan(&start, &[target], None, gripper_angle);
            let elapsed = started.elapsed().as_micros();
            let outcome = match plan {
                Err(refusal) => ReachOutcome::Refused {
                    detail: refusal.detail,
                },
                Ok(motion) => match (motion.pose_limited, motion.path_limited) {
                    (Some(limited), _) => ReachOutcome::PoseLimited {
                        reason: limited.reason,
                    },
                    (None, Some(limited)) => ReachOutcome::PathLimited {
                        reason: limited.reason,
                    },
                    (None, None) => {
                        let last_corner =
                            motion.plan.segments.last().unwrap().corners.last().unwrap();
                        let achieved = planner.describe(last_corner).unwrap();
                        let position_error_meters = (nalgebra::Vector3::from(achieved.position)
                            - nalgebra::Vector3::from(pose.position))
                        .norm();
                        let orientation_error_radians =
                            super::super::orientation::from_xyzw(achieved.quaternion)
                                .angle_to(&super::super::orientation::from_xyzw(pose.quaternion));
                        ReachOutcome::Planned {
                            position_error_meters,
                            orientation_error_radians,
                        }
                    }
                },
            };
            (Some(elapsed), outcome)
        } else {
            (None, ReachOutcome::ReferenceBlocked)
        };
        serde_json::to_writer(
            &mut *output,
            &ReachCase {
                profile,
                seed,
                index,
                start,
                reference_joints,
                target_position: pose.position,
                target_quaternion: pose.quaternion,
                gripper_angle,
                plan_microseconds,
                result,
            },
        )
        .unwrap();
        writeln!(output).unwrap();
    }
}

#[test]
#[ignore = "generates a deterministic reachability benchmark artifact"]
fn reachability_sweep() {
    let output_path =
        std::env::var_os("RUA_REACH_SWEEP_OUTPUT").expect("set RUA_REACH_SWEEP_OUTPUT");
    let mut output = LineWriter::new(std::fs::File::create(output_path).unwrap());
    run_profile(
        &mut output,
        "self_collision",
        &so101_self_collision(),
        421,
        20_000,
        None,
    );
    let table_planner = so101_planner(&json!({
        "self_collision": {"penetration_tolerance_meters": 0.01},
        "surface_guard": {"surface_height_meters": 0.0, "leeway_meters": 0.01}
    }));
    run_profile(
        &mut output,
        "table_open_gripper",
        &table_planner,
        422,
        5_000,
        Some(1.5),
    );
    run_profile(&mut output, "held_out", &table_planner, 9_183, 5_000, None);
    output.flush().unwrap();
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum ImpossibleTarget {
    BeyondReach,
    IncompatibleOrientation,
}

#[derive(Serialize)]
struct ImpossiblePoseTiming {
    target: ImpossibleTarget,
    index: usize,
    plan_microseconds: u128,
    preview_microseconds: u128,
    plan_reaches_target: bool,
    preview_reaches_target: bool,
}

#[test]
#[ignore = "measures planner and preview latency for impossible pose requests"]
fn impossible_pose_latency() {
    use super::ProjectionOutcome;

    let output_path =
        std::env::var_os("RUA_REACH_SWEEP_OUTPUT").expect("set RUA_REACH_SWEEP_OUTPUT");
    let mut output = LineWriter::new(std::fs::File::create(output_path).unwrap());
    let planner = so101_self_collision();
    let mut sampler = JointSampler(6_384);
    let cases = [
        (ImpossibleTarget::BeyondReach, [0.9, 0.0, 0.16]),
        (ImpossibleTarget::IncompatibleOrientation, [0.24, 0.0, 0.16]),
    ];
    let quarter_turn_component = std::f64::consts::FRAC_1_SQRT_2;
    let orientation = PoseOrientation::Quaternion {
        quaternion: [quarter_turn_component, 0.0, 0.0, quarter_turn_component],
    };
    for index in 0..128 {
        let start = sampler.sample(&planner);
        if planner.path_check(&start, &start, None).unwrap() != PathCheck::Clear {
            continue;
        }
        for (target, position) in cases {
            let started = Instant::now();
            let plan = planner.plan(
                &start,
                &[Waypoint::Pose {
                    position,
                    orientation,
                }],
                None,
                None,
            );
            let plan_microseconds = started.elapsed().as_micros();
            let plan_reaches_target = plan
                .is_ok_and(|motion| motion.path_limited.is_none() && motion.pose_limited.is_none());

            let started = Instant::now();
            let preview = planner.project(&start, position, Some(orientation), None);
            let preview_microseconds = started.elapsed().as_micros();
            let preview_reaches_target = matches!(
                preview.outcome,
                ProjectionOutcome::Projected {
                    within_tolerance: true,
                    ..
                }
            );
            assert!(!plan_reaches_target && !preview_reaches_target);
            serde_json::to_writer(
                &mut output,
                &ImpossiblePoseTiming {
                    target,
                    index,
                    plan_microseconds,
                    preview_microseconds,
                    plan_reaches_target,
                    preview_reaches_target,
                },
            )
            .unwrap();
            writeln!(output).unwrap();
        }
    }
    output.flush().unwrap();
}

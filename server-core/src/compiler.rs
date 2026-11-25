use std::collections::{BTreeMap, BTreeSet, VecDeque};

use hebbian_contracts::{
    CompilationDiagnostic, CompiledArmAction, CompiledGoal, CompiledOperation,
    CompiledSequenceWaypoint, MinimumSectionDuration,
};
pub(crate) use hebbian_contracts::{CompilationTarget, CompiledProgram, CompiledStep};
use sha2::{Digest, Sha256};

use hebbian_contracts::{
    ActionSequenceArmAction, ActionSequenceGoal, ActionSequenceRequest, ActionSequenceStep,
    ActionSequenceWaypoint, GripperOpening, JointSpeed,
};

use crate::motion_values::is_valid_trajectory_limit;

pub(crate) fn requested_joints(action: &CompiledArmAction) -> Option<&[f64]> {
    match action {
        CompiledArmAction::Move {
            goal: CompiledGoal::Joints { joints },
            ..
        } => Some(joints),
        CompiledArmAction::Move {
            goal: CompiledGoal::Pose { .. },
            ..
        }
        | CompiledArmAction::Gripper { .. } => None,
    }
}

#[derive(Default)]
pub(crate) struct RobotUseCompiler {
    operations: BTreeMap<SourceLocation, CachedOperation>,
    retention_order: VecDeque<SourceLocation>,
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
struct SourceLocation {
    step: usize,
    arm: String,
}

struct CachedOperation {
    source_operation_id: String,
    target: CompilationTarget,
    source: ActionSequenceArmAction,
    compiled: CompiledArmAction,
}

impl RobotUseCompiler {
    pub fn compile(
        &mut self,
        request: &ActionSequenceRequest,
        targets: &BTreeMap<String, CompilationTarget>,
    ) -> Result<CompiledProgram, CompilationDiagnostic> {
        compile_program(request, targets, self)
    }
}

fn compile_program(
    request: &ActionSequenceRequest,
    targets: &BTreeMap<String, CompilationTarget>,
    compilation_cache: &mut RobotUseCompiler,
) -> Result<CompiledProgram, CompilationDiagnostic> {
    if request.steps.len() > 1024 {
        return Err(CompilationDiagnostic {
            step: None,
            arm: None,
            detail: "A robot program may contain at most 1024 steps".into(),
        });
    }
    if request.steps.is_empty() {
        return Err(CompilationDiagnostic {
            step: None,
            arm: None,
            detail: "An action sequence must contain at least one step".into(),
        });
    }
    let mut reused_operations = 0;
    let mut source_identity = Vec::new();
    let steps = request
        .steps
        .iter()
        .enumerate()
        .map(|(step_index, step)| {
            check_step_shape(step_index, step)?;
            step.arms
                .iter()
                .map(|(arm, action)| {
                    let locate = |detail: &str| CompilationDiagnostic {
                        step: Some(step_index),
                        arm: Some(arm.clone()),
                        detail: detail.to_owned(),
                    };
                    let target = targets.get(arm).ok_or_else(|| locate("Unknown arm"))?;
                    let location = SourceLocation {
                        step: step_index,
                        arm: arm.clone(),
                    };
                    let cached = compilation_cache
                        .operations
                        .get(&location)
                        .filter(|cached| cached.target == *target && cached.source == *action);
                    let (compiled, source_operation_id) = if let Some(cached) = cached {
                        reused_operations += 1;
                        (cached.compiled.clone(), cached.source_operation_id.clone())
                    } else {
                        let compiled = compile_arm_action(action.clone()).map_err(locate)?;
                        check_support(&compiled, target).map_err(locate)?;
                        if !compilation_cache.operations.contains_key(&location) {
                            if compilation_cache.operations.len() >= 2048
                                && let Some(expired) = compilation_cache.retention_order.pop_front()
                            {
                                compilation_cache.operations.remove(&expired);
                            }
                            compilation_cache
                                .retention_order
                                .push_back(location.clone());
                        }
                        source_identity.clear();
                        source_identity.reserve(128);
                        serde_json::to_writer(&mut source_identity, &(step_index, arm, action))
                            .map_err(|_| locate("Cannot identify source operation"))?;
                        let source_operation_id = format!("{:x}", Sha256::digest(&source_identity));
                        compilation_cache.operations.insert(
                            location,
                            CachedOperation {
                                source_operation_id: source_operation_id.clone(),
                                target: target.clone(),
                                source: action.clone(),
                                compiled: compiled.clone(),
                            },
                        );
                        (compiled, source_operation_id)
                    };
                    Ok((
                        arm.clone(),
                        CompiledOperation {
                            source_operation_id,
                            action: compiled,
                        },
                    ))
                })
                .collect()
        })
        .collect::<Result<Vec<CompiledStep>, CompilationDiagnostic>>()?;
    let arms = steps
        .iter()
        .flat_map(BTreeMap::keys)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .cloned()
        .collect();
    Ok(CompiledProgram {
        steps,
        arms,
        reused_operations,
    })
}

fn check_step_shape(
    step_index: usize,
    step: &ActionSequenceStep,
) -> Result<(), CompilationDiagnostic> {
    if step.arms.is_empty() {
        return Err(CompilationDiagnostic {
            step: Some(step_index),
            arm: None,
            detail: "An action sequence step must contain at least one arm".into(),
        });
    }
    if step.arms.len() > 1
        && let Some((arm, _)) = step.arms.iter().find(|(_, action)| {
            matches!(
                action,
                ActionSequenceArmAction::Move {
                    at_seconds: Some(_),
                    ..
                }
            )
        })
    {
        return Err(CompilationDiagnostic {
            step: Some(step_index),
            arm: Some(arm.clone()),
            detail: "Timing requires a step with one planned arm".into(),
        });
    }
    Ok(())
}

fn check_support(
    action: &CompiledArmAction,
    target: &CompilationTarget,
) -> Result<(), &'static str> {
    match action {
        CompiledArmAction::Move {
            goal,
            gripper_opening,
            waypoints,
            minimum_duration_seconds,
            max_joint_acceleration,
            max_joint_jerk,
            ..
        } => {
            for goal in waypoints
                .iter()
                .map(|waypoint| &waypoint.at)
                .chain(std::iter::once(goal))
            {
                match goal {
                    CompiledGoal::Joints { joints } => {
                        if !target.supports_joints {
                            return Err("This arm does not support joint movements");
                        }
                        if joints.len() != target.joint_count
                            || joints.iter().any(|joint| !joint.is_finite())
                        {
                            return Err(
                                "Joint targets must be finite and match the configured joint count",
                            );
                        }
                    }
                    CompiledGoal::Pose {
                        position,
                        orientation,
                    } => {
                        if !target.supports_pose {
                            return Err("This arm does not support pose movements");
                        }
                        if position.iter().any(|coordinate| !coordinate.is_finite()) {
                            return Err("Pose positions must be finite");
                        }
                        crate::orientation::pose_orientation_rotation(
                            *orientation,
                            target.jaw_closing_axis,
                        )
                        .map_err(|_| "Invalid pose orientation")?;
                    }
                }
            }
            // WHY: Station targets expose joint moves only when they use the trajectory planner.
            if !target.supports_joints {
                if minimum_duration_seconds.is_some() {
                    return Err("Timing requires a planned move");
                }
                if !waypoints.is_empty() {
                    return Err("Continuous moves require a planned move");
                }
                if max_joint_acceleration.is_some() || max_joint_jerk.is_some() {
                    return Err("Per-move acceleration and jerk require an IK move");
                }
            }
            if (gripper_opening.is_some()
                || waypoints
                    .iter()
                    .any(|point| point.gripper_opening.is_some()))
                && !target.has_gripper
            {
                return Err("This arm has no gripper");
            }
        }
        CompiledArmAction::Gripper { .. } if !target.has_gripper => {
            return Err("This arm has no gripper");
        }
        CompiledArmAction::Gripper { .. } => {}
    }
    Ok(())
}

fn compile_arm_action(action: ActionSequenceArmAction) -> Result<CompiledArmAction, &'static str> {
    let speed = |requested: Option<f64>| {
        requested
            .map(JointSpeed::new)
            .transpose()
            .map_err(|_| "A joint speed must be positive and finite")
    };
    let opening = |requested| {
        GripperOpening::try_from(requested)
            .map_err(|_| "A gripper opening must be finite and between 0 and 1")
    };
    match action {
        ActionSequenceArmAction::Move {
            waypoints,
            goal,
            at_seconds,
            gripper_opening,
            max_joint_speed,
            max_joint_acceleration,
            max_joint_jerk,
        } => {
            let (waypoints, minimum_duration_seconds) = compile_move_timing(waypoints, at_seconds)?;
            let (goal, movement_speed) = match goal {
                ActionSequenceGoal::Joints {
                    joints,
                    max_joint_speed,
                } => (CompiledGoal::Joints { joints }, max_joint_speed),
                ActionSequenceGoal::Pose {
                    position,
                    orientation,
                    max_joint_speed,
                } => (
                    CompiledGoal::Pose {
                        position,
                        orientation,
                    },
                    max_joint_speed,
                ),
            };
            let max_joint_speed = speed(max_joint_speed.or(movement_speed))?;
            if !max_joint_acceleration.is_none_or(is_valid_trajectory_limit) {
                return Err("A joint acceleration must be positive and finite");
            }
            if !max_joint_jerk.is_none_or(is_valid_trajectory_limit) {
                return Err("A joint jerk must be positive and finite");
            }
            Ok(CompiledArmAction::Move {
                waypoints,
                goal,
                minimum_duration_seconds,
                gripper_opening: gripper_opening.map(opening).transpose()?,
                max_joint_speed,
                max_joint_acceleration,
                max_joint_jerk,
            })
        }
        ActionSequenceArmAction::Gripper {
            gripper_opening,
            max_joint_speed,
        } => {
            let max_joint_speed = speed(max_joint_speed)?;
            Ok(CompiledArmAction::Gripper {
                opening: opening(gripper_opening)?,
                max_joint_speed,
            })
        }
    }
}

fn compile_move_timing(
    waypoints: Vec<ActionSequenceWaypoint>,
    goal_seconds: Option<f64>,
) -> Result<
    (
        Vec<CompiledSequenceWaypoint>,
        Option<MinimumSectionDuration>,
    ),
    &'static str,
> {
    let mut previous_arrival_seconds = None;
    let mut compile_arrival = |arrival: Option<f64>| {
        if arrival.is_some() != goal_seconds.is_some() {
            return Err("Set at_seconds on every waypoint and the goal, or omit all times");
        }
        let Some(arrival_seconds) = arrival else {
            return Ok(None);
        };
        if !arrival_seconds.is_finite() || arrival_seconds < 0.0 {
            return Err("at_seconds must be finite and non-negative");
        }
        if previous_arrival_seconds.is_some_and(|previous| arrival_seconds <= previous) {
            return Err("at_seconds must strictly increase from waypoint to goal");
        }
        let section_seconds = arrival_seconds - previous_arrival_seconds.unwrap_or(0.0);
        previous_arrival_seconds = Some(arrival_seconds);
        MinimumSectionDuration::try_from(section_seconds).map(Some)
    };
    let compiled_waypoints = waypoints
        .into_iter()
        .map(|waypoint| {
            compile_arrival(waypoint.at_seconds).map(|minimum_duration_seconds| {
                CompiledSequenceWaypoint {
                    at: waypoint.at,
                    gripper_opening: waypoint.gripper_opening,
                    minimum_duration_seconds,
                }
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok((compiled_waypoints, compile_arrival(goal_seconds)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn timing_requires_a_complete_increasing_schedule_on_one_arm() {
        let target = CompilationTarget {
            configuration_identity: "config".into(),
            joint_count: 1,
            jaw_closing_axis: hebbian_contracts::JawClosingAxis::X,
            supports_joints: true,
            supports_pose: false,
            has_gripper: true,
        };
        let targets = BTreeMap::from([("arm".into(), target.clone()), ("other".into(), target)]);
        let request = |waypoint_seconds, goal_seconds| ActionSequenceRequest {
            steps: vec![hebbian_contracts::ActionSequenceStep::single(
                "arm",
                ActionSequenceArmAction::Move {
                    goal: ActionSequenceGoal::Joints {
                        joints: vec![0.2],
                        max_joint_speed: None,
                    },
                    waypoints: vec![ActionSequenceWaypoint {
                        at: CompiledGoal::Joints { joints: vec![0.1] },
                        at_seconds: waypoint_seconds,
                        gripper_opening: None,
                    }],
                    at_seconds: goal_seconds,
                    gripper_opening: None,
                    max_joint_speed: None,
                    max_joint_acceleration: None,
                    max_joint_jerk: None,
                },
            )],
            expected_revisions: BTreeMap::new(),
        };
        let mut compiler = RobotUseCompiler::default();
        for (waypoint_seconds, goal_seconds, message) in [
            (None, Some(1.0), "every waypoint"),
            (Some(0.5), None, "every waypoint"),
            (Some(1.0), Some(1.0), "strictly increase"),
            (Some(2.0), Some(1.0), "strictly increase"),
            (Some(-0.1), Some(1.0), "non-negative"),
            (Some(0.5), Some(f64::INFINITY), "finite"),
        ] {
            let error = compiler
                .compile(&request(waypoint_seconds, goal_seconds), &targets)
                .unwrap_err();
            assert_eq!(error.step, Some(0));
            assert!(error.detail.contains(message));
        }
        compiler
            .compile(&request(Some(0.0), Some(1.0)), &targets)
            .unwrap();
        let mut timed = request(Some(0.25), Some(1.0));
        let program = compiler.compile(&timed, &targets).unwrap();
        let CompiledArmAction::Move {
            waypoints,
            minimum_duration_seconds,
            ..
        } = &program.steps[0]["arm"].action
        else {
            panic!("Expected a compiled move");
        };
        assert_eq!(
            waypoints[0].minimum_duration_seconds.map(f64::from),
            Some(0.25)
        );
        assert_eq!(minimum_duration_seconds.map(f64::from), Some(0.75));
        timed.steps[0].arms.insert(
            "other".into(),
            ActionSequenceArmAction::Gripper {
                gripper_opening: 0.5,
                max_joint_speed: None,
            },
        );
        assert!(
            compiler
                .compile(&timed, &targets)
                .unwrap_err()
                .detail
                .contains("one planned arm")
        );
    }

    #[test]
    fn a_pose_only_arm_rejects_planned_move_options_during_compilation() {
        let targets = BTreeMap::from([(
            "policy".into(),
            CompilationTarget {
                configuration_identity: "policy-config".into(),
                joint_count: 5,
                jaw_closing_axis: hebbian_contracts::JawClosingAxis::X,
                supports_joints: false,
                supports_pose: true,
                has_gripper: true,
            },
        )]);
        let goal = ActionSequenceGoal::Pose {
            position: [0.2, 0.0, 0.1],
            orientation: hebbian_contracts::PoseOrientation::Quaternion {
                quaternion: [0.0, 0.0, 0.0, 1.0],
            },
            max_joint_speed: None,
        };
        let request =
            |at_seconds, waypoints, max_joint_acceleration, max_joint_jerk| ActionSequenceRequest {
                steps: vec![hebbian_contracts::ActionSequenceStep::single(
                    "policy",
                    ActionSequenceArmAction::Move {
                        goal: goal.clone(),
                        waypoints,
                        at_seconds,
                        gripper_opening: None,
                        max_joint_speed: None,
                        max_joint_acceleration,
                        max_joint_jerk,
                    },
                )],
                expected_revisions: BTreeMap::new(),
            };
        let mut compiler = RobotUseCompiler::default();
        let untimed = request(None, vec![], None, None);
        compiler.compile(&untimed, &targets).unwrap();
        let waypoint = ActionSequenceWaypoint {
            at: CompiledGoal::Pose {
                position: [0.15, 0.0, 0.1],
                orientation: hebbian_contracts::PoseOrientation::Quaternion {
                    quaternion: [0.0, 0.0, 0.0, 1.0],
                },
            },
            at_seconds: None,
            gripper_opening: None,
        };
        for (mut unsupported, detail) in [
            (
                request(Some(1.0), vec![], None, None),
                "Timing requires a planned move",
            ),
            (
                request(None, vec![waypoint], None, None),
                "Continuous moves require a planned move",
            ),
            (
                request(None, vec![], Some(2.0), None),
                "Per-move acceleration and jerk require an IK move",
            ),
            (
                request(None, vec![], None, Some(2.0)),
                "Per-move acceleration and jerk require an IK move",
            ),
        ] {
            unsupported.steps.insert(0, untimed.steps[0].clone());
            let diagnostic = compiler.compile(&unsupported, &targets).unwrap_err();
            assert_eq!(diagnostic.step, Some(1));
            assert_eq!(diagnostic.arm.as_deref(), Some("policy"));
            assert_eq!(diagnostic.detail, detail);
        }
    }

    #[test]
    fn revisions_reuse_only_unchanged_operations_with_matching_configuration() {
        let mut compiler = RobotUseCompiler::default();
        let action = |opening| ActionSequenceArmAction::Gripper {
            gripper_opening: opening,
            max_joint_speed: None,
        };
        let mut request = ActionSequenceRequest {
            steps: vec![
                hebbian_contracts::ActionSequenceStep::single("arm", action(0.5)),
                hebbian_contracts::ActionSequenceStep::single("arm", action(0.8)),
            ],
            expected_revisions: BTreeMap::new(),
        };
        let mut targets = BTreeMap::from([(
            "arm".into(),
            CompilationTarget {
                configuration_identity: "first-config".into(),
                joint_count: 1,
                jaw_closing_axis: hebbian_contracts::JawClosingAxis::X,
                supports_joints: true,
                supports_pose: true,
                has_gripper: true,
            },
        )]);
        let original = compiler.compile(&request, &targets).unwrap();
        assert_eq!(original.reused_operations, 0);
        request.steps[1] = hebbian_contracts::ActionSequenceStep::single("arm", action(0.2));
        let edited = compiler.compile(&request, &targets).unwrap();
        assert_eq!(edited.reused_operations, 1);
        assert_eq!(
            original.steps[0]["arm"].source_operation_id,
            edited.steps[0]["arm"].source_operation_id
        );
        assert_ne!(
            original.steps[1]["arm"].source_operation_id,
            edited.steps[1]["arm"].source_operation_id
        );
        assert_eq!(
            edited.steps[1]["arm"].action,
            CompiledArmAction::Gripper {
                opening: GripperOpening::try_from(0.2).unwrap(),
                max_joint_speed: None,
            }
        );
        targets.get_mut("arm").unwrap().configuration_identity = "second-config".into();
        assert_eq!(
            compiler
                .compile(&request, &targets)
                .unwrap()
                .reused_operations,
            0
        );
        targets.get_mut("arm").unwrap().has_gripper = false;
        let diagnostic = compiler.compile(&request, &targets).unwrap_err();
        assert_eq!(diagnostic.step, Some(0));
        assert_eq!(diagnostic.arm.as_deref(), Some("arm"));
        assert_eq!(diagnostic.detail, "This arm has no gripper");
    }

    #[test]
    fn invalid_target_identifies_the_later_step_and_arm() {
        let request = ActionSequenceRequest {
            steps: vec![
                hebbian_contracts::ActionSequenceStep::single(
                    "left",
                    ActionSequenceArmAction::Gripper {
                        gripper_opening: 0.5,
                        max_joint_speed: None,
                    },
                ),
                hebbian_contracts::ActionSequenceStep::single(
                    "right",
                    ActionSequenceArmAction::Gripper {
                        gripper_opening: 1.5,
                        max_joint_speed: None,
                    },
                ),
            ],
            expected_revisions: BTreeMap::new(),
        };
        let targets = ["left", "right"]
            .into_iter()
            .map(|arm| {
                (
                    arm.into(),
                    CompilationTarget {
                        configuration_identity: "fixture".into(),
                        joint_count: 1,
                        jaw_closing_axis: hebbian_contracts::JawClosingAxis::X,
                        supports_joints: true,
                        supports_pose: true,
                        has_gripper: true,
                    },
                )
            })
            .collect();
        let diagnostic = RobotUseCompiler::default()
            .compile(&request, &targets)
            .err()
            .unwrap();
        assert_eq!(diagnostic.step, Some(1));
        assert_eq!(diagnostic.arm.as_deref(), Some("right"));
        assert_eq!(
            diagnostic.to_string(),
            "Step 2, arm right: A gripper opening must be finite and between 0 and 1"
        );
    }

    #[test]
    fn invalid_motion_caps_in_a_later_step_reject_the_whole_sequence() {
        let move_action = |max_joint_acceleration, max_joint_jerk| ActionSequenceArmAction::Move {
            at_seconds: None,
            waypoints: vec![],
            goal: ActionSequenceGoal::Joints {
                joints: vec![0.1],
                max_joint_speed: None,
            },
            gripper_opening: None,
            max_joint_speed: None,
            max_joint_acceleration,
            max_joint_jerk,
        };
        let step = |acceleration, jerk| {
            hebbian_contracts::ActionSequenceStep::single("arm", move_action(acceleration, jerk))
        };
        let targets = BTreeMap::from([(
            "arm".into(),
            CompilationTarget {
                configuration_identity: "fixture".into(),
                joint_count: 1,
                jaw_closing_axis: hebbian_contracts::JawClosingAxis::X,
                supports_joints: true,
                supports_pose: true,
                has_gripper: false,
            },
        )]);
        let mut request = ActionSequenceRequest {
            steps: vec![step(None, None), step(Some(2.0), Some(10.0))],
            expected_revisions: BTreeMap::new(),
        };
        let mut compiler = RobotUseCompiler::default();
        assert!(compiler.compile(&request, &targets).is_ok());

        for invalid in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            for (acceleration, jerk, quantity) in [
                (Some(invalid), None, "acceleration"),
                (None, Some(invalid), "jerk"),
            ] {
                request.steps[1] = step(acceleration, jerk);
                let diagnostic = compiler.compile(&request, &targets).unwrap_err();
                assert_eq!(diagnostic.step, Some(1));
                assert_eq!(diagnostic.arm.as_deref(), Some("arm"));
                assert_eq!(
                    diagnostic.detail,
                    format!("A joint {quantity} must be positive and finite")
                );
            }
        }
    }

    #[test]
    fn both_resources_use_the_same_effective_step_speed() {
        let step = |gripper_speed| ActionSequenceArmAction::Move {
            at_seconds: None,
            waypoints: vec![],
            goal: ActionSequenceGoal::Joints {
                joints: vec![0.1],
                max_joint_speed: Some(0.7),
            },
            gripper_opening: Some(0.5),
            max_joint_speed: gripper_speed,
            max_joint_acceleration: None,
            max_joint_jerk: None,
        };
        for (requested, expected) in [(None, 0.7), (Some(0.3), 0.3)] {
            let compiled = compile_arm_action(step(requested)).unwrap();
            assert!(matches!(
                compiled,
                CompiledArmAction::Move {
                    goal: CompiledGoal::Joints { .. },
                    gripper_opening: Some(_),
                    max_joint_speed: Some(speed),
                    ..
                } if (speed.radians_per_second() - expected).abs() < f64::EPSILON
            ));
        }
    }
}

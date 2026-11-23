use std::collections::{BTreeMap, BTreeSet, VecDeque};

use hebbian_contracts::{
    CompilationDiagnostic, CompiledArmAction, CompiledGoal, CompiledOperation,
};
pub(crate) use hebbian_contracts::{CompilationTarget, CompiledProgram, CompiledStep};
use sha2::{Digest, Sha256};

use hebbian_contracts::{
    ActionSequenceArmAction, ActionSequenceGoal, ActionSequenceRequest, GripperOpening, JointSpeed,
};

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
            if step.arms.is_empty() {
                return Err(CompilationDiagnostic {
                    step: Some(step_index),
                    arm: None,
                    detail: "An action sequence step must contain at least one arm".into(),
                });
            }
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

fn check_support(
    action: &CompiledArmAction,
    target: &CompilationTarget,
) -> Result<(), &'static str> {
    match action {
        CompiledArmAction::Move {
            goal,
            gripper_opening,
            ..
        } => {
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
                    crate::planning::orientation::pose_orientation_rotation(
                        *orientation,
                        target.jaw_closing_axis,
                    )
                    .map_err(|_| "Invalid pose orientation")?;
                }
            }
            if gripper_opening.is_some() && !target.has_gripper {
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
            goal,
            gripper_opening,
            max_joint_speed,
        } => {
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
            Ok(CompiledArmAction::Move {
                goal,
                gripper_opening: gripper_opening.map(opening).transpose()?,
                max_joint_speed,
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

#[cfg(test)]
mod tests {
    use super::*;
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
    fn both_resources_use_the_same_effective_step_speed() {
        let step = |gripper_speed| ActionSequenceArmAction::Move {
            goal: ActionSequenceGoal::Joints {
                joints: vec![0.1],
                max_joint_speed: Some(0.7),
            },
            gripper_opening: Some(0.5),
            max_joint_speed: gripper_speed,
        };
        for (requested, expected) in [(None, 0.7), (Some(0.3), 0.3)] {
            let compiled = compile_arm_action(step(requested)).unwrap();
            assert!(matches!(
                compiled,
                CompiledArmAction::Move {
                    goal: CompiledGoal::Joints { .. },
                    gripper_opening: Some(_),
                    max_joint_speed: Some(speed),
                } if (speed.radians_per_second() - expected).abs() < f64::EPSILON
            ));
        }
    }
}

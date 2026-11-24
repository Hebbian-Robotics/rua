use std::collections::BTreeMap;

use super::submissions::ControllerStreamCommand;
use crate::action_step::{PreparedArmAction, PreparedGripper, PreparedMovement};
use crate::config::ControllerConfig;
use crate::control_link::LinkHandle;
use crate::planning::Planner;
use hebbian_contracts::{
    ActionSequenceWaypoint, ArmMode, Command, CompiledArmAction, CompiledGoal, GripperOpening,
    JointSpeed, MoveArmsCommand, PlannedMotion, PoseOrientation, PoseProjection, PreviewPose,
    RejectionReason, Sample, StationCommand, Waypoint,
};

use crate::arm::{Rejection, rejection};

const FIRST_SAMPLE_WAIT: std::time::Duration = std::time::Duration::from_secs(1);

fn latest_sample(link: &LinkHandle) -> Result<Sample, Rejection> {
    link.wait_for_sample(FIRST_SAMPLE_WAIT)
        .ok_or_else(|| rejection(RejectionReason::ArmBusy, "No feedback from the arm yet"))
}

#[derive(Clone, Copy)]
struct HoldingFeedback<'a> {
    pose: &'a [f64],
    gripper_angle: Option<f64>,
}

impl<'a> TryFrom<&'a Sample> for HoldingFeedback<'a> {
    type Error = Rejection;

    fn try_from(sample: &'a Sample) -> Result<Self, Rejection> {
        match (sample.mode, sample.hold_pose.as_deref()) {
            (ArmMode::Holding, Some(pose)) => Ok(Self {
                pose,
                gripper_angle: sample.gripper_angle,
            }),
            (ArmMode::Disabled, _) => Err(rejection(
                RejectionReason::NotEnabled,
                "The arm is disabled; send enable, then repeat the command",
            )),
            (ArmMode::Faulted, _) => Err(rejection(
                RejectionReason::Faulted,
                "The arm is faulted; send clear_fault after inspecting it",
            )),
            _ => Err(rejection(
                RejectionReason::ArmBusy,
                "Another command is active; send stop first",
            )),
        }
    }
}

fn jaw_angle(
    link: &LinkHandle,
    arm: &str,
    opening: hebbian_contracts::GripperOpening,
) -> Result<hebbian_contracts::JawAngle, Rejection> {
    let gripper = link.gripper().ok_or_else(|| {
        rejection(
            RejectionReason::InvalidTarget,
            format!("{arm} has no gripper"),
        )
    })?;
    hebbian_contracts::JawAngle::try_from(gripper.angle(f64::from(opening)))
        .map_err(|error| rejection(RejectionReason::InvalidTarget, error.to_string()))
}

fn planner_or_refuse(planner: Option<&Planner>) -> Result<&Planner, Rejection> {
    planner.ok_or_else(|| {
        rejection(
            RejectionReason::InvalidTarget,
            "This arm has no tool link to plan for",
        )
    })
}

fn plan_trajectory(
    planner: &Planner,
    holding: HoldingFeedback<'_>,
    waypoints: &[Waypoint],
    max_joint_speed: Option<JointSpeed>,
) -> Result<PlannedMotion, Rejection> {
    planner.plan(
        holding.pose,
        waypoints,
        max_joint_speed,
        holding.gripper_angle,
    )
}

fn planned_speed(motion: &PlannedMotion) -> Result<JointSpeed, Rejection> {
    JointSpeed::new(motion.plan.peak_speed)
        .map_err(|error| rejection(RejectionReason::InvalidTarget, error.to_string()))
}

fn prepare_trajectory(label: &str, motion: PlannedMotion) -> Result<PreparedMovement, Rejection> {
    Ok(PreparedMovement::Trajectory {
        label: label.to_owned(),
        joint_speed: planned_speed(&motion)?,
        plan: motion.plan,
        path_limited: motion.path_limited,
        pose_limited: motion.pose_limited,
    })
}

const POLICY_POSES_ONLY: &str = "With learned control, send one pose at a time using move_to_pose";

enum StationMovement {
    Rehome,
    Trajectory {
        label: &'static str,
        waypoints: Vec<Waypoint>,
        max_joint_speed: Option<JointSpeed>,
    },
    Pose {
        position: [f64; 3],
        orientation: PoseOrientation,
        max_joint_speed: Option<JointSpeed>,
    },
}

impl StationMovement {
    fn joints(joints: Vec<f64>, max_joint_speed: Option<JointSpeed>) -> Self {
        Self::Trajectory {
            label: "move_to_joints",
            waypoints: vec![Waypoint::Joints { joints }],
            max_joint_speed,
        }
    }

    fn rejects_policy(&self) -> bool {
        matches!(self, Self::Rehome | Self::Trajectory { .. })
    }
}

#[deny(clippy::disallowed_methods)]
fn plan_station_movement(
    movement: StationMovement,
    config: &ControllerConfig,
    planner: Option<&Planner>,
    uses_policy: bool,
    sample: &Sample,
) -> Result<PreparedMovement, Rejection> {
    let (label, waypoints, max_joint_speed, final_gripper_angle) = match movement {
        StationMovement::Pose {
            position,
            orientation,
            ..
        } if uses_policy => {
            HoldingFeedback::try_from(sample)?;
            return Ok(PreparedMovement::PolicyPose {
                label: "move_to_pose".to_owned(),
                position,
                orientation,
            });
        }
        StationMovement::Pose {
            position,
            orientation,
            max_joint_speed,
        } => (
            "move_to_pose",
            vec![Waypoint::Pose {
                position,
                orientation,
            }],
            max_joint_speed,
            None,
        ),
        StationMovement::Trajectory {
            label,
            waypoints,
            max_joint_speed,
        } => (label, waypoints, max_joint_speed, None),
        StationMovement::Rehome => {
            let rest_pose = sample.rest_pose.as_deref().ok_or_else(|| {
                rejection(
                    RejectionReason::NotEnabled,
                    "The arm has no saved home pose; enable it first",
                )
            })?;
            let joints = config.recovery_limits().clamp(rest_pose).map_err(|error| {
                rejection(
                    RejectionReason::InvalidTarget,
                    format!("Saved home pose: {error}"),
                )
            })?;
            let speed = JointSpeed::new(config.max_joint_speed.min(2.0))
                .map_err(|error| rejection(RejectionReason::InvalidTarget, error.to_string()))?;
            let gripper_angle = sample
                .rest_gripper_angle
                .filter(|_| config.gripper.is_some())
                .map(hebbian_contracts::JawAngle::try_from)
                .transpose()
                .map_err(|error| rejection(RejectionReason::InvalidTarget, error.to_string()))?;
            (
                "rehome",
                vec![Waypoint::Joints { joints }],
                Some(speed),
                gripper_angle,
            )
        }
    };
    let planner = planner_or_refuse(planner)?;
    let holding = HoldingFeedback::try_from(sample)?;
    let mut motion = plan_trajectory(planner, holding, &waypoints, max_joint_speed)?;
    if motion.path_limited.is_none()
        && motion.pose_limited.is_none()
        && let Some(final_segment) = motion.plan.segments.last_mut()
    {
        final_segment.final_gripper_angle = final_gripper_angle;
    }
    prepare_trajectory(label, motion)
}

fn prepare_movement(
    link: &LinkHandle,
    movement: StationMovement,
) -> Result<PreparedMovement, Rejection> {
    let uses_policy = link.has_policy();
    if uses_policy && movement.rejects_policy() {
        return Err(rejection(RejectionReason::InvalidTarget, POLICY_POSES_ONLY));
    }
    plan_station_movement(
        movement,
        link.config(),
        link.planner(),
        uses_policy,
        &latest_sample(link)?,
    )
}

fn prepare_movement_with_limits(
    link: &LinkHandle,
    movement: StationMovement,
    acceleration: Option<f64>,
    jerk: Option<f64>,
) -> Result<PreparedMovement, Rejection> {
    let mut prepared = prepare_movement(link, movement)?;
    if acceleration.is_some() || jerk.is_some() {
        let PreparedMovement::Trajectory { plan, .. } = &mut prepared else {
            return Err(rejection(
                RejectionReason::InvalidTarget,
                "Per-move acceleration and jerk require an IK move",
            ));
        };
        plan.trajectory_limits = Some(resolve_trajectory_limits(
            link.config(),
            acceleration,
            jerk,
        )?);
    }
    Ok(prepared)
}

fn resolve_trajectory_limits(
    config: &ControllerConfig,
    acceleration: Option<f64>,
    jerk: Option<f64>,
) -> Result<hebbian_contracts::TrajectoryLimits, Rejection> {
    crate::motion_values::resolve_trajectory_limits(
        config.motion.trajectory_limits(),
        acceleration,
        jerk,
    )
    .map_err(|error| rejection(RejectionReason::InvalidTarget, error.to_string()))
}

fn prepare_gripper(
    link: &LinkHandle,
    arm: &str,
    opening: GripperOpening,
    max_joint_speed: Option<JointSpeed>,
) -> Result<PreparedGripper, Rejection> {
    Ok(PreparedGripper {
        angle: jaw_angle(link, arm, opening)?,
        joint_speed: max_joint_speed,
    })
}

pub(crate) fn arm_command(
    link: &LinkHandle,
    arm: &str,
    command: StationCommand,
) -> Result<Command, Rejection> {
    let movement = match command {
        StationCommand::Enable => return Ok(Command::Enable),
        StationCommand::ApproveRecovery => return Ok(Command::ApproveRecovery),
        StationCommand::Disable => return Ok(Command::Disable),
        StationCommand::Stop => return Ok(Command::Stop),
        StationCommand::ClearFault => return Ok(Command::ClearFault),
        StationCommand::Rehome => StationMovement::Rehome,
        StationCommand::MoveToJoints {
            joints,
            max_joint_speed,
            max_joint_acceleration,
            max_joint_jerk,
        } => {
            return prepare_movement_with_limits(
                link,
                StationMovement::joints(joints, max_joint_speed),
                max_joint_acceleration,
                max_joint_jerk,
            )
            .map(PreparedMovement::into_command);
        }
        StationCommand::MoveToPose {
            position,
            orientation,
            max_joint_speed,
            max_joint_acceleration,
            max_joint_jerk,
        } => {
            return prepare_movement_with_limits(
                link,
                StationMovement::Pose {
                    position,
                    orientation,
                    max_joint_speed,
                },
                max_joint_acceleration,
                max_joint_jerk,
            )
            .map(PreparedMovement::into_command);
        }
        StationCommand::StreamJoints {
            joints,
            max_joint_speed,
            gripper_opening,
        } => {
            return joint_stream(
                link,
                arm,
                joints,
                gripper_opening,
                StreamAdmission::Start { max_joint_speed },
            );
        }
        StationCommand::SetGripper {
            opening,
            max_joint_speed,
        } => {
            return prepare_gripper(link, arm, opening, max_joint_speed)
                .map(PreparedGripper::into_command);
        }
    };
    prepare_movement(link, movement).map(PreparedMovement::into_command)
}

fn prepare_waypoint_crossings(
    link: &LinkHandle,
    arm: &str,
    movement: &mut PreparedMovement,
    waypoints: &[ActionSequenceWaypoint],
    gripper_opening: Option<GripperOpening>,
) -> Result<(), Rejection> {
    let PreparedMovement::Trajectory {
        plan,
        path_limited,
        pose_limited,
        ..
    } = movement
    else {
        return Err(rejection(RejectionReason::InvalidTarget, POLICY_POSES_ONLY));
    };
    if path_limited.is_some() || pose_limited.is_some() {
        return Err(rejection(
            RejectionReason::InvalidTarget,
            "Continuous move cannot reach its requested path",
        ));
    }
    for (segment, point) in plan.segments.iter_mut().zip(waypoints) {
        segment.pass_through = Some(hebbian_contracts::PassThrough {
            gripper_angle: point
                .gripper_opening
                .map(|opening| jaw_angle(link, arm, opening))
                .transpose()?,
        });
    }
    let initial_jaw_goal = gripper_opening
        .map(|opening| jaw_angle(link, arm, opening))
        .transpose()?
        .map(hebbian_contracts::JawAngle::radians);
    planner_or_refuse(link.planner())?
        .check_continuous_path(
            plan,
            link.config(),
            latest_sample(link)?.gripper_angle,
            initial_jaw_goal,
        )
        .map_err(|error| rejection(RejectionReason::InvalidTarget, format!("{error:#}")))?;
    Ok(())
}

pub(super) fn prepare_compiled_action(
    link: &LinkHandle,
    arm: &str,
    action: &CompiledArmAction,
) -> Result<PreparedArmAction, Rejection> {
    match action {
        CompiledArmAction::Move {
            waypoints,
            goal,
            gripper_opening,
            max_joint_speed,
            max_joint_acceleration,
            max_joint_jerk,
        } => {
            let movement = if waypoints.is_empty() {
                match goal {
                    CompiledGoal::Joints { joints } => {
                        StationMovement::joints(joints.clone(), *max_joint_speed)
                    }
                    CompiledGoal::Pose {
                        position,
                        orientation,
                    } => StationMovement::Pose {
                        position: *position,
                        orientation: *orientation,
                        max_joint_speed: *max_joint_speed,
                    },
                }
            } else {
                StationMovement::Trajectory {
                    label: "continuous_move",
                    waypoints: waypoints
                        .iter()
                        .map(|point| &point.at)
                        .chain(std::iter::once(goal))
                        .map(|point| match point {
                            CompiledGoal::Pose {
                                position,
                                orientation,
                            } => Waypoint::Pose {
                                position: *position,
                                orientation: *orientation,
                            },
                            CompiledGoal::Joints { joints } => Waypoint::Joints {
                                joints: joints.clone(),
                            },
                        })
                        .collect(),
                    max_joint_speed: *max_joint_speed,
                }
            };
            let mut movement = prepare_movement_with_limits(
                link,
                movement,
                *max_joint_acceleration,
                *max_joint_jerk,
            )?;
            if !waypoints.is_empty() {
                prepare_waypoint_crossings(link, arm, &mut movement, waypoints, *gripper_opening)?;
            }
            let action = match gripper_opening {
                Some(opening) => {
                    let gripper = prepare_gripper(link, arm, *opening, *max_joint_speed)?;
                    PreparedArmAction::move_and_gripper(arm.to_owned(), movement, gripper)
                }
                None => PreparedArmAction::movement(arm.to_owned(), movement),
            };
            Ok(action)
        }
        CompiledArmAction::Gripper {
            opening,
            max_joint_speed,
        } => {
            let gripper = prepare_gripper(link, arm, *opening, *max_joint_speed)?;
            Ok(PreparedArmAction::gripper(arm.to_owned(), gripper))
        }
    }
}

pub(super) enum StreamAdmission {
    Start {
        max_joint_speed: Option<hebbian_contracts::JointSpeed>,
    },
    Update(ControllerStreamCommand),
}

pub(super) fn joint_stream(
    link: &LinkHandle,
    arm: &str,
    positions: Vec<f64>,
    gripper_opening: Option<hebbian_contracts::GripperOpening>,
    admission: StreamAdmission,
) -> Result<Command, Rejection> {
    if link.has_policy() {
        return Err(rejection(RejectionReason::InvalidTarget, POLICY_POSES_ONLY));
    }
    let gripper_angle = gripper_opening
        .map(|opening| jaw_angle(link, arm, opening))
        .transpose()?;
    let (joint_speed, stream_command_id) = match admission {
        StreamAdmission::Start { max_joint_speed } => (max_joint_speed, None),
        StreamAdmission::Update(stream) => (None, Some(stream.into_controller_id())),
    };
    Ok(Command::StreamJoints {
        gripper_angle,
        positions,
        joint_speed,
        stream_command_id,
    })
}

pub(crate) fn plan_group_move<'a>(
    links: &'a BTreeMap<String, LinkHandle>,
    command: &MoveArmsCommand,
    expected_revisions: &BTreeMap<String, u64>,
) -> Result<
    (
        &'a LinkHandle,
        Vec<PreparedArmAction>,
        Vec<crate::lease::ArmGate>,
    ),
    Rejection,
> {
    let mut moves = Vec::new();
    let mut gates = Vec::new();
    let mut first_link = None;
    for (arm, waypoint) in &command.goals {
        let link = links.get(arm).ok_or_else(|| {
            rejection(
                RejectionReason::InvalidTarget,
                format!("No arm named {arm}"),
            )
        })?;
        if link.has_policy() {
            return Err(rejection(
                RejectionReason::InvalidTarget,
                "Use per-arm pose commands for arms with learned control",
            ));
        }
        let prefixed = |Rejection { reason, detail }| rejection(reason, format!("{arm}: {detail}"));
        let sample = latest_sample(link).map_err(prefixed)?;
        let planner = planner_or_refuse(link.planner()).map_err(prefixed)?;
        let holding = HoldingFeedback::try_from(&sample).map_err(prefixed)?;
        let motion = plan_trajectory(
            planner,
            holding,
            std::slice::from_ref(waypoint),
            command.max_joint_speed,
        )
        .map_err(prefixed)?;
        let joint_speed = planned_speed(&motion).map_err(prefixed)?;
        let PlannedMotion {
            plan,
            path_limited,
            pose_limited,
        } = motion;
        gates.push(link.gate(expected_revisions.get(arm).copied()));
        moves.push(PreparedArmAction::movement(
            arm.clone(),
            PreparedMovement::Trajectory {
                label: "move_arms".into(),
                plan,
                joint_speed,
                path_limited,
                pose_limited,
            },
        ));
        first_link.get_or_insert(link);
    }
    let link = first_link
        .ok_or_else(|| rejection(RejectionReason::InvalidTarget, "move_arms names no arm"))?;
    Ok((link, moves, gates))
}

pub fn answer_preview(
    links: &BTreeMap<String, LinkHandle>,
    arm: &str,
    pose: &PreviewPose,
) -> Result<PoseProjection, Rejection> {
    let link = links.get(arm).ok_or_else(|| {
        rejection(
            RejectionReason::InvalidTarget,
            format!("No arm named {arm}"),
        )
    })?;
    let config = link.config();
    let effective = if link.has_policy() {
        if pose.max_joint_acceleration.is_some() || pose.max_joint_jerk.is_some() {
            return Err(rejection(
                RejectionReason::InvalidTarget,
                "Per-move acceleration and jerk require an IK move",
            ));
        }
        None
    } else {
        let speed = JointSpeed::for_request(
            pose.max_joint_speed,
            config.default_joint_speed,
            config.max_joint_speed,
        )
        .map_err(|error| rejection(RejectionReason::InvalidTarget, error.to_string()))?;
        let trajectory =
            resolve_trajectory_limits(config, pose.max_joint_acceleration, pose.max_joint_jerk)?;
        Some(hebbian_contracts::EffectiveMotionLimits::new(
            speed.radians_per_second(),
            trajectory,
        ))
    };
    let sample = latest_sample(link)?;
    let mut projection = link.project(
        pose.position,
        pose.orientation,
        &sample.positions,
        sample.gripper_angle,
    )?;
    projection.effective_motion_limits = effective;
    Ok(projection)
}

pub(crate) fn preview_answer(
    links: &BTreeMap<String, LinkHandle>,
    arm: &str,
    pose: &PreviewPose,
) -> hebbian_contracts::PreviewAnswer {
    match answer_preview(links, arm, pose) {
        Ok(projection) => hebbian_contracts::PreviewAnswer::Projected {
            projection: Box::new(projection),
        },
        Err(Rejection { reason, detail }) => {
            hebbian_contracts::PreviewAnswer::Refused { reason, detail }
        }
    }
}

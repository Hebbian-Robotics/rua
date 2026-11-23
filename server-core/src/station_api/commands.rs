use std::collections::BTreeMap;

use super::submissions::ControllerStreamCommand;
use crate::action_step::{PreparedArmAction, PreparedGripper, PreparedMovement};
use crate::config::ControllerConfig;
use crate::control_link::LinkHandle;
use crate::planning::Planner;
use hebbian_contracts::{
    ArmMode, Command, CompiledArmAction, CompiledMovement, GripperOpening, JointSpeed,
    MoveArmsCommand, PlannedMotion, PoseOrientation, PoseProjection, PreviewPose, RejectionReason,
    Sample, StationCommand, Waypoint,
};

type Refusal = (RejectionReason, String);

fn refused(reason: RejectionReason, detail: impl Into<String>) -> Refusal {
    (reason, detail.into())
}

const FIRST_SAMPLE_WAIT: std::time::Duration = std::time::Duration::from_secs(1);

fn latest_sample(link: &LinkHandle) -> Result<Sample, Refusal> {
    link.wait_for_sample(FIRST_SAMPLE_WAIT)
        .ok_or_else(|| refused(RejectionReason::ArmBusy, "No feedback from the arm yet"))
}

struct HoldingFeedback<'a> {
    pose: &'a [f64],
    gripper_angle: Option<f64>,
}

impl<'a> TryFrom<&'a Sample> for HoldingFeedback<'a> {
    type Error = Refusal;

    fn try_from(sample: &'a Sample) -> Result<Self, Refusal> {
        match (sample.mode, sample.hold_pose.as_deref()) {
            (ArmMode::Holding, Some(pose)) => Ok(Self {
                pose,
                gripper_angle: sample.gripper_angle,
            }),
            (ArmMode::Disabled, _) => Err(refused(
                RejectionReason::NotEnabled,
                "The arm is disabled; send enable, then repeat the command",
            )),
            (ArmMode::Faulted, _) => Err(refused(
                RejectionReason::Faulted,
                "The arm is faulted; send clear_fault after inspecting it",
            )),
            _ => Err(refused(
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
) -> Result<hebbian_contracts::JawAngle, Refusal> {
    let gripper = link.gripper().ok_or_else(|| {
        refused(
            RejectionReason::InvalidTarget,
            format!("{arm} has no gripper"),
        )
    })?;
    hebbian_contracts::JawAngle::try_from(gripper.angle(f64::from(opening)))
        .map_err(|error| refused(RejectionReason::InvalidTarget, error.to_string()))
}

fn planner_or_refuse(planner: Option<&Planner>) -> Result<&Planner, Refusal> {
    planner.ok_or_else(|| {
        refused(
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
) -> Result<PlannedMotion, Refusal> {
    planner
        .plan(
            holding.pose,
            waypoints,
            max_joint_speed,
            holding.gripper_angle,
        )
        .map_err(|refusal| (refusal.reason, refusal.detail))
}

fn planned_speed(motion: &PlannedMotion) -> Result<JointSpeed, Refusal> {
    JointSpeed::new(motion.plan.peak_speed)
        .map_err(|error| refused(RejectionReason::InvalidTarget, error.to_string()))
}

fn prepare_trajectory(label: &str, motion: PlannedMotion) -> Result<PreparedMovement, Refusal> {
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

fn plan_station_movement(
    movement: StationMovement,
    config: &ControllerConfig,
    planner: Option<&Planner>,
    uses_policy: bool,
    sample: &Sample,
) -> Result<PreparedMovement, Refusal> {
    let (label, waypoints, max_joint_speed) = match movement {
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
        ),
        StationMovement::Trajectory {
            label,
            waypoints,
            max_joint_speed,
        } => (label, waypoints, max_joint_speed),
        StationMovement::Rehome => {
            let rest_pose = sample.rest_pose.as_deref().ok_or_else(|| {
                refused(
                    RejectionReason::NotEnabled,
                    "The arm has no saved home pose; enable it first",
                )
            })?;
            let joints = config.recovery_limits().clamp(rest_pose).map_err(|error| {
                refused(
                    RejectionReason::InvalidTarget,
                    format!("Saved home pose: {error}"),
                )
            })?;
            let speed = JointSpeed::new(config.max_joint_speed.min(2.0))
                .map_err(|error| refused(RejectionReason::InvalidTarget, error.to_string()))?;
            ("rehome", vec![Waypoint::Joints { joints }], Some(speed))
        }
    };
    let planner = planner_or_refuse(planner)?;
    let holding = HoldingFeedback::try_from(sample)?;
    plan_trajectory(planner, holding, &waypoints, max_joint_speed)
        .and_then(|motion| prepare_trajectory(label, motion))
}

fn prepare_movement(
    link: &LinkHandle,
    movement: StationMovement,
) -> Result<PreparedMovement, Refusal> {
    let uses_policy = link.has_policy();
    if uses_policy && movement.rejects_policy() {
        return Err(refused(RejectionReason::InvalidTarget, POLICY_POSES_ONLY));
    }
    plan_station_movement(
        movement,
        link.config(),
        link.planner(),
        uses_policy,
        &latest_sample(link)?,
    )
}

fn prepare_gripper(
    link: &LinkHandle,
    arm: &str,
    opening: GripperOpening,
    max_joint_speed: Option<JointSpeed>,
) -> Result<PreparedGripper, Refusal> {
    Ok(PreparedGripper {
        angle: jaw_angle(link, arm, opening)?,
        joint_speed: max_joint_speed,
    })
}

pub(crate) fn arm_command(
    link: &LinkHandle,
    arm: &str,
    command: StationCommand,
) -> Result<Command, Refusal> {
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
        } => StationMovement::joints(joints, max_joint_speed),
        StationCommand::MoveToPose {
            position,
            orientation,
            max_joint_speed,
        } => StationMovement::Pose {
            position,
            orientation,
            max_joint_speed,
        },
        StationCommand::FollowPath {
            waypoints,
            max_joint_speed,
        } => StationMovement::Trajectory {
            label: "follow_path",
            waypoints,
            max_joint_speed,
        },
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

pub(super) fn prepare_compiled_action(
    link: &LinkHandle,
    arm: &str,
    action: &CompiledArmAction,
) -> Result<PreparedArmAction, Refusal> {
    match action {
        CompiledArmAction::Move {
            movement,
            gripper_opening,
            max_joint_speed,
        } => {
            let movement = match movement {
                CompiledMovement::Joints { joints } => {
                    StationMovement::joints(joints.clone(), *max_joint_speed)
                }
                CompiledMovement::Pose {
                    position,
                    orientation,
                } => StationMovement::Pose {
                    position: *position,
                    orientation: *orientation,
                    max_joint_speed: *max_joint_speed,
                },
            };
            let movement = prepare_movement(link, movement)?;
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
) -> Result<Command, Refusal> {
    if link.has_policy() {
        return Err(refused(RejectionReason::InvalidTarget, POLICY_POSES_ONLY));
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
    Refusal,
> {
    let mut moves = Vec::new();
    let mut gates = Vec::new();
    let mut first_link = None;
    for (arm, waypoint) in &command.targets {
        let link = links.get(arm).ok_or_else(|| {
            refused(
                RejectionReason::InvalidTarget,
                format!("No arm named {arm}"),
            )
        })?;
        if link.has_policy() {
            return Err(refused(
                RejectionReason::InvalidTarget,
                "Use per-arm pose commands for arms with learned control",
            ));
        }
        let prefixed = |(reason, detail): Refusal| (reason, format!("{arm}: {detail}"));
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
        .ok_or_else(|| refused(RejectionReason::InvalidTarget, "move_arms names no arm"))?;
    Ok((link, moves, gates))
}

pub fn answer_preview(
    links: &BTreeMap<String, LinkHandle>,
    arm: &str,
    pose: PreviewPose,
) -> Result<PoseProjection, Refusal> {
    let link = links.get(arm).ok_or_else(|| {
        refused(
            RejectionReason::InvalidTarget,
            format!("No arm named {arm}"),
        )
    })?;
    let sample = latest_sample(link)?;
    link.project(
        pose.position,
        pose.orientation,
        &sample.positions,
        sample.gripper_angle,
    )
}

pub(crate) fn preview_answer(
    links: &BTreeMap<String, LinkHandle>,
    arm: &str,
    pose: PreviewPose,
) -> hebbian_contracts::PreviewAnswer {
    match answer_preview(links, arm, pose) {
        Ok(projection) => hebbian_contracts::PreviewAnswer::Projected {
            projection: Box::new(projection),
        },
        Err((reason, detail)) => hebbian_contracts::PreviewAnswer::Refused { reason, detail },
    }
}

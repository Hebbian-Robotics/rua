use std::collections::BTreeMap;

use crate::control_link::LinkHandle;
use hebbian_contracts::{
    ArmMode, ArmMove, Command, MoveArmsCommand, PlannedMotion, PoseProjection, PreviewPose,
    RejectionReason, Sample, StationCommand, Waypoint,
};

type Refusal = (RejectionReason, String);

fn refused(reason: RejectionReason, detail: impl Into<String>) -> Refusal {
    (reason, detail.into())
}

const FIRST_SAMPLE_WAIT: std::time::Duration = std::time::Duration::from_secs(1);

fn latest_sample(link: &LinkHandle) -> Result<Sample, Refusal> {
    let deadline = std::time::Instant::now() + FIRST_SAMPLE_WAIT;
    loop {
        if let Some(sample) = link.latest_sample_blocking() {
            return Ok(sample);
        }
        if std::time::Instant::now() > deadline {
            return Err(refused(
                RejectionReason::ArmBusy,
                "No feedback from the arm yet",
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

fn hold_pose(sample: Sample) -> Result<Vec<f64>, Refusal> {
    match (sample.mode, sample.hold_pose) {
        (ArmMode::Holding, Some(pose)) => Ok(pose),
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

fn jaw_angle(link: &LinkHandle, arm: &str, opening: f64) -> Result<f64, Refusal> {
    if !opening.is_finite() {
        return Err(refused(
            RejectionReason::InvalidTarget,
            "A gripper opening must be a finite number from 0 (closed) to 1 (open)",
        ));
    }
    let opening = opening.clamp(0.0, 1.0);
    link.gripper()
        .map(|gripper| gripper.angle(opening))
        .ok_or_else(|| {
            refused(
                RejectionReason::InvalidTarget,
                format!("{arm} has no gripper"),
            )
        })
}

fn planned_move(
    link: &LinkHandle,
    waypoints: &[Waypoint],
    max_joint_speed: Option<f64>,
) -> Result<PlannedMotion, Refusal> {
    let planner = link.planner().ok_or_else(|| {
        refused(
            RejectionReason::InvalidTarget,
            "This arm has no tool link to plan for",
        )
    })?;
    let sample = latest_sample(link)?;
    let gripper_angle = sample.gripper_angle;
    let start = hold_pose(sample)?;
    planner
        .plan(&start, waypoints, max_joint_speed, gripper_angle)
        .map_err(|refusal| (refusal.reason, refusal.detail))
}

fn move_command(label: &str, motion: PlannedMotion) -> Command {
    Command::Move {
        label: label.to_owned(),
        joint_speed: motion.plan.peak_speed,
        plan: motion.plan,
        path_limited: motion.path_limited,
        pose_limited: motion.pose_limited,
    }
}

const POLICY_POSES_ONLY: &str =
    "The policy controller takes one pose at a time; send move_to_pose, not joints or a path";

pub(crate) fn arm_command(
    link: &LinkHandle,
    arm: &str,
    command: StationCommand,
) -> Result<Command, Refusal> {
    let policy = link.has_policy();
    match command {
        StationCommand::Enable => Ok(Command::Enable),
        StationCommand::ApproveRecovery => Ok(Command::ApproveRecovery),
        StationCommand::Disable => Ok(Command::Disable),
        StationCommand::Stop => Ok(Command::Stop),
        StationCommand::ClearFault => Ok(Command::ClearFault),
        StationCommand::MoveToJoints {
            joints,
            max_joint_speed,
        } => {
            if policy {
                return Err(refused(RejectionReason::InvalidTarget, POLICY_POSES_ONLY));
            }
            planned_move(link, &[Waypoint::Joints { joints }], max_joint_speed)
                .map(|motion| move_command("move_to_joints", motion))
        }
        StationCommand::MoveToPose {
            position,
            orientation,
            max_joint_speed,
        } => {
            if policy {
                hold_pose(latest_sample(link)?)?;
                return Ok(Command::ReachPose {
                    label: "move_to_pose".to_owned(),
                    position,
                    orientation,
                });
            }
            planned_move(
                link,
                &[Waypoint::Pose {
                    position,
                    orientation,
                }],
                max_joint_speed,
            )
            .map(|motion| move_command("move_to_pose", motion))
        }
        StationCommand::FollowPath {
            waypoints,
            max_joint_speed,
        } => {
            if policy {
                return Err(refused(RejectionReason::InvalidTarget, POLICY_POSES_ONLY));
            }
            planned_move(link, &waypoints, max_joint_speed)
                .map(|motion| move_command("follow_path", motion))
        }
        StationCommand::StreamJoints {
            joints,
            stream_command_id,
            max_joint_speed,
            gripper_opening,
        } => {
            if policy {
                return Err(refused(RejectionReason::InvalidTarget, POLICY_POSES_ONLY));
            }
            let gripper_angle = gripper_opening
                .map(|opening| jaw_angle(link, arm, opening))
                .transpose()?;
            Ok(Command::StreamJoints {
                gripper_angle,
                positions: joints,
                joint_speed: max_joint_speed,
                stream_command_id,
            })
        }
        StationCommand::SetGripper {
            opening,
            max_joint_speed,
        } => Ok(Command::SetGripper {
            angle: jaw_angle(link, arm, opening)?,
            joint_speed: max_joint_speed,
        }),
    }
}

pub(crate) fn plan_group_move<'a>(
    links: &'a BTreeMap<String, LinkHandle>,
    command: &MoveArmsCommand,
    expected_revisions: &BTreeMap<String, u64>,
) -> Result<(&'a LinkHandle, Vec<ArmMove>, Vec<crate::lease::ArmGate>), Refusal> {
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
                "Use per-arm pose commands with the policy controller",
            ));
        }
        let prefixed = |(reason, detail): Refusal| (reason, format!("{arm}: {detail}"));
        let motion = planned_move(
            link,
            std::slice::from_ref(waypoint),
            command.max_joint_speed,
        )
        .map_err(prefixed)?;
        let joint_speed = motion.plan.peak_speed;
        let PlannedMotion {
            plan,
            path_limited,
            pose_limited,
        } = motion;
        gates.push(link.gate(expected_revisions.get(arm).copied()));
        moves.push(ArmMove {
            arm: arm.clone(),
            plan,
            joint_speed,
            path_limited,
            pose_limited,
        });
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

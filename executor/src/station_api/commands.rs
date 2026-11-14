//! Client-level commands turned into the arm's own: motions planned from where
//! the arm holds now, openings turned into jaw angles, pose commands handed to
//! the policy on a policy arm. Each call waits for the loop, so callers run it
//! off the async runtime.

use std::collections::BTreeMap;
use std::sync::{Arc, atomic::AtomicU64};

use crate::control_link::LinkHandle;
use hebbian_contracts::{
    ArmMode, ArmMove, ArmRecord, Command, CommandAnswer, MoveArmsCommand, PoseProjection,
    PreviewPose, RejectionReason, Sample, StationCommand, TraceContext, Waypoint,
};

type Refusal = (RejectionReason, String);

fn refused(reason: RejectionReason, detail: impl Into<String>) -> Refusal {
    (reason, detail.into())
}

fn answer_of(result: Result<Vec<ArmRecord>, Refusal>) -> CommandAnswer {
    match result {
        Ok(records) => CommandAnswer::Accepted { records },
        Err((reason, detail)) => CommandAnswer::Refused { reason, detail },
    }
}

/// How long a command waits for the arm's first sample after startup.
const FIRST_SAMPLE_WAIT: std::time::Duration = std::time::Duration::from_secs(1);

/// The arm's newest sample, waiting briefly for the first one.
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

/// Where the arm holds, which a plan starts from; refused unless it holds.
fn hold_pose(link: &LinkHandle) -> Result<Vec<f64>, Refusal> {
    let sample = latest_sample(link)?;
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
    // Past either end means that end.
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

/// The arm's move along these waypoints, planned from where it holds.
fn planned_move(
    link: &LinkHandle,
    label: &str,
    waypoints: &[Waypoint],
    max_joint_speed: Option<f64>,
) -> Result<Command, Refusal> {
    let planner = link.planner().ok_or_else(|| {
        refused(
            RejectionReason::InvalidTarget,
            "This arm has no tool link to plan for",
        )
    })?;
    let start = hold_pose(link)?;
    let motion = planner
        .plan(&start, waypoints, max_joint_speed)
        .map_err(|refusal| (refusal.reason, refusal.detail))?;
    Ok(Command::Move {
        label: label.to_owned(),
        joint_speed: motion.plan.peak_speed,
        plan: motion.plan,
        path_limited: motion.path_limited,
        pose_limited: motion.pose_limited,
    })
}

const POLICY_POSES_ONLY: &str =
    "The policy controller takes one pose at a time; send move_to_pose, not joints or a path";

/// The arm's own command for a client's.
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
            planned_move(
                link,
                "move_to_joints",
                &[Waypoint::Joints { joints }],
                max_joint_speed,
            )
        }
        StationCommand::MoveToPose {
            position,
            orientation,
            max_joint_speed,
        } => {
            if policy {
                hold_pose(link)?;
                return Ok(Command::ReachPose {
                    label: "move_to_pose".to_owned(),
                    position,
                    orientation,
                });
            }
            planned_move(
                link,
                "move_to_pose",
                &[Waypoint::Pose {
                    position,
                    orientation,
                }],
                max_joint_speed,
            )
        }
        StationCommand::FollowPath {
            waypoints,
            max_joint_speed,
        } => {
            if policy {
                return Err(refused(RejectionReason::InvalidTarget, POLICY_POSES_ONLY));
            }
            planned_move(link, "follow_path", &waypoints, max_joint_speed)
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

/// Carry out one client command on one arm as the holder of `token`.
#[must_use]
pub fn answer_command(
    links: &BTreeMap<String, LinkHandle>,
    arm: &str,
    command: StationCommand,
    token: Option<&str>,
    trace_context: Option<TraceContext>,
    expected_revision: Option<u64>,
    generation: Vec<(Arc<AtomicU64>, u64)>,
) -> CommandAnswer {
    answer_of((|| {
        let link = links.get(arm).ok_or_else(|| {
            refused(
                RejectionReason::InvalidTarget,
                format!("No arm named {arm}"),
            )
        })?;
        let command = arm_command(link, arm, command)?;
        let record = link.submit_command_fenced(
            command,
            token,
            trace_context,
            expected_revision,
            generation,
        )?;
        Ok(vec![ArmRecord {
            arm: arm.to_owned(),
            record,
        }])
    })())
}

/// Plan a coordinated move without admitting it to the control loop.
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
        let Command::Move {
            plan,
            joint_speed,
            path_limited,
            pose_limited,
            ..
        } = planned_move(
            link,
            "move_arms",
            std::slice::from_ref(waypoint),
            command.max_joint_speed,
        )
        .map_err(prefixed)?
        else {
            unreachable!("planned_move makes a move");
        };
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

/// Where the arm would take the tool toward a pose from its measured joints.
///
/// # Errors
/// The arm is unknown, has not reported yet, or has no tool link.
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

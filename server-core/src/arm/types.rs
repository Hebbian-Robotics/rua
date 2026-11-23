use std::time::Instant;

use crate::motion_values::JointSpeed;
use hebbian_contracts::{ArmMode, JawClosingAxis, PathLimited, PoseLimited, RejectionReason};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Rejection {
    pub reason: RejectionReason,
    pub detail: String,
}

pub(crate) fn rejection(reason: RejectionReason, detail: impl Into<String>) -> Rejection {
    Rejection {
        reason,
        detail: detail.into(),
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ShortOfTarget {
    pub path_limited: Option<PathLimited>,
    pub pose_limited: Option<PoseLimited>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PoseGoalSettings {
    pub position_tolerance_meters: f64,
    pub orientation_tolerance_radians: f64,
}

#[derive(Debug, Clone)]
pub struct Limits {
    pub joint_count: usize,
    pub lower: Vec<f64>,
    pub upper: Vec<f64>,
    pub joint_limit_recovery_tolerance_radians: f64,
    pub gripper: Option<(f64, f64)>,
    pub pose_goals: Option<PoseGoalSettings>,
    pub jaw_closing_axis: JawClosingAxis,
    pub default_joint_speed: f64,
    pub max_joint_speed: f64,
}

// CONSTRAINT: Use wall time for records and monotonic time for deadlines; callers supply both.
#[derive(Debug, Clone, Copy)]
pub struct Now {
    pub unix_nano: hebbian_contracts::UnixNanos,
    pub instant: Instant,
}

impl Now {
    #[must_use]
    pub(crate) fn current() -> Self {
        Self {
            unix_nano: hebbian_contracts::UnixNanos::now(),
            instant: Instant::now(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Job {
    Enable {
        command_id: hebbian_contracts::CommandId,
    },
    ApproveRecovery {
        command_id: hebbian_contracts::CommandId,
    },
    Release {
        command_id: hebbian_contracts::CommandId,
    },
    Move {
        command_id: hebbian_contracts::CommandId,
        plan: super::AdmittedPlan,
        group: Option<GroupMembership>,
    },
    Reach {
        command_id: hebbian_contracts::CommandId,
        goal: ReachGoal,
    },
    Stream {
        command_id: hebbian_contracts::CommandId,
    },
}

impl Job {
    #[must_use]
    pub(crate) fn command_id(&self) -> &hebbian_contracts::CommandId {
        match self {
            Self::Enable { command_id }
            | Self::ApproveRecovery { command_id }
            | Self::Release { command_id }
            | Self::Move { command_id, .. }
            | Self::Reach { command_id, .. }
            | Self::Stream { command_id } => command_id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GroupMembership {
    pub id: crate::motion_values::GroupId,
    pub arms: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReachGoal {
    pub position: [f64; 3],
    pub quaternion: [f64; 4],
    pub position_tolerance_meters: f64,
    pub orientation_tolerance_radians: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum StreamGoal {
    Joints {
        positions: crate::joint_limits::JointTarget<Vec<f64>>,
        joint_speed: JointSpeed,
    },
    Pose {
        position: [f64; 3],
        quaternion: [f64; 4],
    },
}

impl StreamGoal {
    pub(super) fn label(&self) -> &'static str {
        match self {
            Self::Joints { .. } => "stream_joints",
            Self::Pose { .. } => "stream_pose",
        }
    }

    pub(super) fn recorded(&self) -> hebbian_contracts::RecordedCommand {
        use hebbian_contracts::{CommandPose, RecordedCommand};
        match self {
            Self::Joints {
                positions,
                joint_speed,
            } => RecordedCommand::JointStream {
                goal_joints: positions.as_slice().to_vec(),
                joint_speed: joint_speed.radians_per_second(),
            },
            Self::Pose {
                position,
                quaternion,
            } => RecordedCommand::PoseStream {
                goal_pose: CommandPose {
                    position: *position,
                    quaternion: *quaternion,
                },
            },
        }
    }

    pub(super) fn joint_speed(&self) -> Option<JointSpeed> {
        match self {
            Self::Joints { joint_speed, .. } => Some(*joint_speed),
            Self::Pose { .. } => None,
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct HeldStreamGoal {
    pub(super) command_id: hebbian_contracts::CommandId,
    pub(super) goal: StreamGoal,
    pub(super) last_goal_at: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct GripperTarget {
    pub(crate) angle: hebbian_contracts::JawAngle,
    pub(crate) speed: crate::motion_values::JointSpeed,
}

pub(super) struct GripperCommand {
    pub(super) command_id: hebbian_contracts::CommandId,
    pub(super) pending_target: Option<GripperTarget>,
    pub(super) stream_id: Option<hebbian_contracts::CommandId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ShutdownState {
    NotRequested,
    Stopping,
    ReturningToRest,
}

pub(super) struct ArmFault {
    pub(super) detail: String,
    pub(super) entered_at: Instant,
}

pub(super) enum PowerState {
    Disabled,
    Holding(Poses),
    Moving(Poses),
    Faulted(ArmFault),
    Disconnected(ArmFault),
    Reconnected,
}

impl PowerState {
    pub(super) fn mode(&self) -> ArmMode {
        match self {
            Self::Disabled => ArmMode::Disabled,
            Self::Holding(_) => ArmMode::Holding,
            Self::Moving(_) => ArmMode::Moving,
            Self::Faulted(_) => ArmMode::Faulted,
            Self::Disconnected(_) => ArmMode::Disconnected,
            Self::Reconnected => ArmMode::Reconnected,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum CommandState {
    Idle,
    Queued(Job),
    // INVARIANT: The previous motion has been superseded, but the loop still owns it.
    // The replacement can be cancelled before the loop lets go.
    Superseding(Option<Job>),
    Starting(hebbian_contracts::CommandId),
    Executing {
        command_id: hebbian_contracts::CommandId,
        settling: bool,
    },
    Stopping(hebbian_contracts::CommandId),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct Poses {
    // SAFETY: Where the arm rested unpowered; shutdown returns there before releasing.
    pub rest: Vec<f64>,
    pub hold: Vec<f64>,
}

#[derive(Debug, Clone)]
pub(crate) struct Observation {
    pub sampled_at: Now,
    pub positions: Vec<f64>,
    pub tool_pose: Option<([f64; 3], [f64; 4])>,
    pub fault: Option<String>,
}

//! The arm's records and the values the state machine trades in.

use std::time::Instant;

use hebbian_contracts::{
    ArmMode, CornerPlan, JawClosingAxis, PathLimited, PoseLimited, RejectionReason,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct Rejection {
    pub reason: RejectionReason,
    pub detail: String,
}

pub(super) fn rejection(reason: RejectionReason, detail: impl Into<String>) -> Rejection {
    Rejection {
        reason,
        detail: detail.into(),
    }
}

/// Why a planned move stops short of what was asked for, if it does.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ShortOfTarget {
    pub path_limited: Option<PathLimited>,
    pub pose_limited: Option<PoseLimited>,
}

/// When a pose goal counts as reached.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PoseGoalSettings {
    pub position_tolerance_meters: f64,
    pub orientation_tolerance_radians: f64,
}

/// What the arm's admission rules need from the executor's config.
#[derive(Debug, Clone)]
pub struct Limits {
    pub joint_count: usize,
    pub lower: Vec<f64>,
    pub upper: Vec<f64>,
    pub joint_limit_recovery_tolerance_radians: f64,
    /// The jaw's angle range; none when the arm has no gripper.
    pub gripper: Option<(f64, f64)>,
    /// How a pose goal counts as reached; none when the arm has no policy to
    /// reach one with.
    pub pose_goals: Option<PoseGoalSettings>,
    /// The tool-frame axis the jaws close along, which jaw yaw is measured by.
    pub jaw_closing_axis: JawClosingAxis,
    pub default_joint_speed: f64,
    pub max_joint_speed: f64,
}

/// When a call happens: wall-clock seconds for records, a monotonic instant for
/// deadlines. The caller supplies both so tests can control them.
#[derive(Debug, Clone, Copy)]
pub struct Now {
    pub unix_seconds: f64,
    pub instant: Instant,
}

impl Now {
    #[must_use]
    pub(crate) fn current() -> Self {
        Self {
            unix_seconds: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0.0, |elapsed| elapsed.as_secs_f64()),
            instant: Instant::now(),
        }
    }
}

/// What the loop should do next, in the order commands were accepted.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Job {
    Enable {
        command_id: String,
    },
    ApproveRecovery {
        command_id: String,
    },
    Release {
        command_id: String,
    },
    Move {
        command_id: String,
        plan: CornerPlan,
        /// Set for one arm's part of a grouped motion.
        group: Option<GroupMembership>,
    },
    Reach {
        command_id: String,
        goal: ReachGoal,
    },
    Stream {
        command_id: String,
    },
}

impl Job {
    #[must_use]
    pub(crate) fn command_id(&self) -> &str {
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

/// The arms moving together in one grouped motion, in the order their joints are
/// combined into the shared trajectory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GroupMembership {
    pub id: String,
    pub arms: Vec<String>,
}

/// Where a reach-and-finish command is going, and how close counts.
#[derive(Debug, Clone, PartialEq)]
pub struct ReachGoal {
    pub position: [f64; 3],
    pub quaternion: [f64; 4],
    pub position_tolerance_meters: f64,
    pub orientation_tolerance_radians: f64,
}

/// The goal a stream follows now.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum StreamGoal {
    Joints {
        positions: Vec<f64>,
        joint_speed: f64,
    },
    Pose {
        position: [f64; 3],
        quaternion: [f64; 4],
    },
}

#[derive(Debug, Clone)]
pub(super) struct HeldStreamGoal {
    pub(super) command_id: String,
    pub(super) goal: StreamGoal,
    pub(super) last_goal_at: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct GripperTarget {
    pub(crate) angle_radians: f64,
    pub(crate) max_speed_radians_per_second: f64,
}

pub(super) struct GripperCommand {
    pub(super) command_id: String,
    pub(super) pending_target: Option<GripperTarget>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ShutdownState {
    NotRequested,
    Stopping,
    ReturningToRest,
}

pub(super) struct ArmFault {
    pub(super) detail: String,
    pub(super) entered_at: f64,
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

/// The one command occupying the arm's motion channel.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Lifecycle {
    Idle,
    Queued(Job),
    /// The previous motion has been superseded, but the loop still owns it.
    /// The replacement can be cancelled before the loop lets go.
    Superseding(Option<Job>),
    Starting(String),
    Executing {
        command_id: String,
        settling: bool,
    },
    Stopping(String),
}

/// Poses that exist only while the motors are enabled.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct Poses {
    /// Where the arm rested unpowered; shutdown returns there before releasing.
    pub rest: Vec<f64>,
    /// Where the arm holds between motions; plans start here.
    pub hold: Vec<f64>,
}

/// What the loop last saw, for finishing records and for `clear_fault`.
#[derive(Debug, Clone)]
pub(crate) struct Observation {
    pub sampled_at: f64,
    pub positions: Vec<f64>,
    pub tool_pose: Option<([f64; 3], [f64; 4])>,
    /// A fault the plant reports; clear while every motor is healthy.
    pub fault: Option<String>,
}

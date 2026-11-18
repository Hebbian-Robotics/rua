//! The motion types clients send and read: commands, plans, orientations,
//! projections, inverse-kinematics results and control answers.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{PathLimited, PoseLimited};

/// Above this absolute normalized approach X, jaw yaw zero uses world Y.
pub const JAW_REFERENCE_SWITCH_APPROACH_X: f64 = 0.95;

/// Why a command was not accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RejectionReason {
    ArmBusy,
    NotEnabled,
    // An interlock was raised on the arm since the caller's `expected_revision`.
    StaleRevision,
    // An interlock on the arm refuses motion.
    Interlocked,
    Faulted,
    InvalidTarget,
    StreamEnded,
    // No joints put the tool at the pose.
    Unreachable,
    // Another client holds the lease.
    LeaseHeldElsewhere,
    // The request was not a readable request at all.
    Malformed,
}

/// Client to arm.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Command {
    /// Run the preflight checks, then turn torque on holding the present pose.
    #[schemars(title = "EnableCommand")]
    Enable,
    /// Approve recovery of an interrupted arm after supporting it.
    #[schemars(title = "ApproveRecoveryCommand")]
    ApproveRecovery,
    /// Turn torque off, which drops the arm.
    #[schemars(title = "DisableCommand")]
    Disable,
    /// End the active motion and any queued commands, and hold where the arm is.
    #[schemars(title = "StopCommand")]
    Stop,
    /// Acknowledge a fault once feedback shows every motor healthy and disabled.
    #[schemars(title = "ClearFaultCommand")]
    ClearFault,
    /// Return to the rest pose and disable, as the station does when it shuts down.
    #[schemars(title = "ShutdownCommand")]
    Shutdown,
    /// Follow a planned path of straight joint-space segments between corners,
    /// starting from the pose the arm holds.
    #[schemars(title = "MoveCommand")]
    Move {
        /// What the client calls this kind of move (`move_to_pose`, ...).
        label: String,
        plan: CornerPlan,
        /// The speed the plan was made at, after clamping; recorded.
        joint_speed: f64,
        path_limited: Option<PathLimited>,
        pose_limited: Option<PoseLimited>,
    },
    /// Track these joint positions; later goals carry the id the first returned.
    #[schemars(title = "StreamJointsCommand")]
    StreamJoints {
        /// The gripper's newest target, applied together with the joints.
        gripper_angle: Option<f64>,
        positions: Vec<f64>,
        joint_speed: Option<f64>,
        stream_command_id: Option<String>,
    },
    /// Reach this tool pose with the learned policy; later goals carry the id.
    #[schemars(title = "StreamPoseCommand")]
    StreamPose {
        position: [f64; 3],
        quaternion: [f64; 4],
        stream_command_id: Option<String>,
    },
    /// Reach this tool pose with the learned policy and finish: the command ends
    /// once the tool has stayed within both tolerances and the arm has come to
    /// rest, or times out.
    #[schemars(title = "ReachPoseCommand")]
    ReachPose {
        /// What the client calls this kind of move (`move_to_pose`).
        label: String,
        position: [f64; 3],
        orientation: PoseOrientation,
    },
    /// Move the jaw to this angle (the URDF gripper joint's).
    #[schemars(title = "SetGripperCommand")]
    SetGripper {
        angle: f64,
        joint_speed: Option<f64>,
    },
}

/// One straight run between corners, with where it is meant to end.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, PartialEq)]
pub struct Segment {
    pub label: String,
    pub corners: Vec<Vec<f64>>,
    /// Slows the section to last at least this long, so arms moving together
    /// can arrive together. Only for a single straight section.
    #[serde(default)]
    pub minimum_duration_seconds: Option<f64>,
    #[serde(default)]
    pub target_position: Option<[f64; 3]>,
    #[serde(default)]
    pub target_quaternion: Option<[f64; 4]>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, PartialEq)]
pub struct CornerPlan {
    pub segments: Vec<Segment>,
    pub peak_speed: f64,
}

/// One corner of a move: joints, or a tool pose solved for joints.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Waypoint {
    #[schemars(title = "JointsWaypoint")]
    Joints { joints: Vec<f64> },
    #[schemars(title = "PoseWaypoint")]
    Pose {
        position: [f64; 3],
        orientation: PoseOrientation,
    },
}

/// What the arm is asked to run, and why it stops short of the target if it does.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PlannedMotion {
    pub plan: CornerPlan,
    pub path_limited: Option<PathLimited>,
    pub pose_limited: Option<PoseLimited>,
}

/// A named approach direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NamedApproach {
    /// World -Z.
    Down,
    /// World +X.
    Forward,
}

/// The direction the jaws point along: a named one, or a nonzero world direction.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Approach {
    Named(NamedApproach),
    #[schemars(title = "ApproachDirection")]
    Direction([f64; 3]),
}

/// A tool orientation as a client gives it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PoseOrientation {
    /// The approach direction and the jaw yaw about it, in degrees.
    #[schemars(title = "GraspOrientation")]
    Grasp {
        approach: Approach,
        jaw_yaw_degrees: f64,
    },
    /// A raw quaternion, `[x, y, z, w]`.
    #[schemars(title = "QuaternionOrientation")]
    Quaternion { quaternion: [f64; 4] },
}

/// An orientation in the terms a pose takes it: the approach and the jaw yaw
/// about it, at full precision, in degrees from 0 up to but excluding 360, so
/// sending them back reproduces the rotation.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ToolOrientation {
    pub approach: [f64; 3],
    pub jaw_yaw_degrees: f64,
}

/// A run of jaw yaws the arm can or cannot take at a position and approach.
/// Degrees follow the jaw yaw convention, from 0 up to 360, in order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct JawYawRange {
    pub from_degrees: f64,
    pub to_degrees: f64,
    pub reachable: bool,
    pub reason: Option<String>,
}

/// Where the arm would put the tool.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ProjectedPose {
    pub joints: Vec<f64>,
    pub position: [f64; 3],
    pub quaternion: [f64; 4],
    pub orientation: ToolOrientation,
    pub position_error_meters: f64,
    /// From the requested orientation; none when the solver chose it.
    pub orientation_error_radians: Option<f64>,
}

/// A usable projected pose, or why the preview could not produce one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProjectionOutcome {
    /// The best pose found, including an approximation when the target was not met.
    #[schemars(title = "ProjectedPoseOutcome")]
    Projected {
        pose: ProjectedPose,
        /// The solver or controller met the target without a path constraint.
        within_tolerance: bool,
        /// Why the target was not met; none when it was met within tolerance.
        reason: Option<String>,
    },
    /// The preview could not produce a usable pose.
    #[schemars(title = "UnavailableProjection")]
    Unavailable { reason: String },
}

/// The orientation the target asked for, so a caller that sent a quaternion
/// learns the approach and jaw yaw it stands for, and one that sent those gets
/// the quaternion to send instead.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RequestedOrientation {
    pub orientation: ToolOrientation,
    pub quaternion: [f64; 4],
}

/// The answer to a projection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PoseProjection {
    pub outcome: ProjectionOutcome,
    /// None for a position-only target the solver chose the orientation for.
    pub requested: Option<RequestedOrientation>,
    /// The jaw yaws the arm can take at the projected position and approach;
    /// empty when nothing was projected or the arm has no wrist roll.
    pub jaw_yaw_ranges: Vec<JawYawRange>,
    pub calculation_milliseconds: f64,
}

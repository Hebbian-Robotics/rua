//! The client-level API: the commands clients and models send and the state
//! they read, over HTTP and its event stream. These types are the only
//! definition; `hebbian-robot-use-server schema --openapi` carries them, and the Python
//! client's and the viewer's types are generated from that.
//!
//! Units are SI (metres, radians, seconds); poses are in the station's world
//! frame. A pose takes a position and an orientation: an approach direction with
//! a jaw yaw in degrees, or a raw `[x, y, z, w]` quaternion. To keep the tool's
//! orientation, send its current quaternion from the state.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{ArmRecord, CommandRecord, PoseOrientation, PoseProjection, RejectionReason, Waypoint};

/// A command for one arm.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum StationCommand {
    /// Preflight, then power the motors holding their present pose.
    #[schemars(title = "EnableCommand")]
    Enable,
    /// Approve recovery after inspecting and supporting the arm.
    #[schemars(title = "ApproveRecoveryCommand")]
    ApproveRecovery,
    /// Turn torque off where the arm is; an unsupported arm will fall.
    #[schemars(title = "DisableCommand")]
    Disable,
    /// End the active motion with outcome stopped and hold where the arm is.
    /// Anyone may send it, whoever holds the lease.
    #[schemars(title = "StopCommand")]
    Stop,
    /// Acknowledge a fault; the arm is already disabled.
    #[schemars(title = "ClearFaultCommand")]
    ClearFault,
    /// Move to joint angles, one per joint in the state's order.
    #[schemars(title = "MoveToJointsCommand")]
    MoveToJoints {
        joints: Vec<f64>,
        /// Peak joint speed in rad/s; clamped to the profile's maximum.
        #[serde(default)]
        max_joint_speed: Option<f64>,
    },
    /// Move the tool to a world position and orientation.
    #[schemars(title = "MoveToPoseCommand")]
    MoveToPose {
        position: [f64; 3],
        orientation: PoseOrientation,
        #[serde(default)]
        max_joint_speed: Option<f64>,
    },
    /// Waypoints are corners: the arm comes to rest and settles at each one.
    #[schemars(title = "FollowPathCommand")]
    FollowPath {
        waypoints: Vec<Waypoint>,
        #[serde(default)]
        max_joint_speed: Option<f64>,
    },
    /// Stream trajectory destinations. The first goal, without
    /// `stream_command_id`, starts a stream when the arm is holding; later goals
    /// carry the command id it returned. Each cycle the arm moves toward the
    /// newest goal at up to the first goal's speed. The stream ends stopped when
    /// goals stop arriving, on stop, or on a trip.
    #[schemars(title = "StationStreamJointsCommand")]
    StreamJoints {
        joints: Vec<f64>,
        #[serde(default)]
        stream_command_id: Option<String>,
        #[serde(default)]
        max_joint_speed: Option<f64>,
        /// The gripper's newest target, 0 closed to 1 open, if the stream drives it.
        #[serde(default)]
        gripper_opening: Option<f64>,
    },
    /// Open or close the gripper, 0 closed to 1 open, whether the arm is holding
    /// or moving. It completes on arrival or on contact.
    #[schemars(title = "StationSetGripperCommand")]
    SetGripper {
        opening: f64,
        /// Peak gripper speed in rad/s; uses the arm default and maximum.
        #[serde(default)]
        max_joint_speed: Option<f64>,
    },
}

/// One movement in an action sequence. The arm settles at each target.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ActionSequenceMovement {
    #[schemars(title = "ActionSequenceJointsMovement")]
    Joints {
        joints: Vec<f64>,
        #[serde(default)]
        max_joint_speed: Option<f64>,
    },
    #[schemars(title = "ActionSequencePoseMovement")]
    Pose {
        position: [f64; 3],
        orientation: PoseOrientation,
        #[serde(default)]
        max_joint_speed: Option<f64>,
    },
}

/// A movement and gripper target in the same step start together.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ActionSequenceStep {
    #[serde(default)]
    pub movement: Option<ActionSequenceMovement>,
    #[serde(default)]
    pub gripper_opening: Option<f64>,
    /// Speed for both resources in rad/s; overrides the movement speed when provided.
    #[serde(default)]
    pub max_joint_speed: Option<f64>,
}

/// Identifies one station process for retry and event-cursor fencing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StationInstance {
    pub id: String,
}

/// A position in one station process's retained event stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StationEventCursor {
    pub station_instance_id: String,
    pub sequence: u64,
}

/// Submit one arm command with a client-generated request identifier.
/// The station uses the identifier to recognize retries of the same submission.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CommandSubmissionRequest {
    /// A caller-generated UUID v4 identifying this submission.
    pub request_id: String,
    /// The server instance the caller observed when creating this request.
    pub station_instance_id: String,
    pub arm: String,
    pub command: StationCommand,
    /// The active lease token, when one is required for this command.
    #[serde(default)]
    pub lease_token: Option<String>,
    #[serde(default)]
    pub expected_revision: Option<u64>,
    #[serde(default)]
    pub trace_context: Option<crate::TraceContext>,
}

/// Submit a coordinated multi-arm move with retry-safe request identity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GroupCommandSubmissionRequest {
    /// A caller-generated UUID v4 identifying this submission.
    pub request_id: String,
    pub station_instance_id: String,
    pub command: MoveArmsCommand,
    #[serde(default)]
    pub lease_token: Option<String>,
    #[serde(default)]
    pub expected_revisions: BTreeMap<String, u64>,
    #[serde(default)]
    pub trace_context: Option<crate::TraceContext>,
}

/// Cancel a submission and stop its affected arms while the lease is current.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CommandCancellationRequest {
    pub request_id: String,
    pub station_instance_id: String,
    pub lease_token: String,
    pub arms: Vec<String>,
}

/// An ordered set of steps to execute under the caller's lease.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ActionSequenceRequest {
    pub arm: String,
    pub steps: Vec<ActionSequenceStep>,
    /// Refuse the action sequence (and stop it between steps) once the arm's revision
    /// moves past this; see `ArmState::revision`.
    #[serde(default)]
    pub expected_revision: Option<u64>,
}

/// Start an action sequence under an explicit station lease.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ActionSequenceStartRequest {
    pub lease_token: String,
    pub action_sequence: ActionSequenceRequest,
}

/// Progress of an action sequence, including the records of each finished step.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ActionSequencePhase {
    #[schemars(title = "ActionSequenceRunning")]
    Running { step: usize },
    #[schemars(title = "ActionSequenceCompleted")]
    Completed,
    #[schemars(title = "ActionSequenceStopped")]
    Stopped { detail: String },
}

/// The station's retained action sequence state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ActionSequenceRecord {
    pub id: String,
    pub arm: String,
    /// The submitted steps, including those not yet executed.
    pub requested_steps: Vec<ActionSequenceStep>,
    pub phase: ActionSequencePhase,
    /// Command records for steps that have finished or been interrupted.
    pub steps: Vec<Vec<CommandRecord>>,
}

/// Move several arms together: every target is planned before any arm moves,
/// one refusal refuses all, and when one arm ends without completing the others
/// stop and hold. No collision checking between the arms.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MoveArmsCommand {
    pub targets: BTreeMap<String, Waypoint>,
    #[serde(default)]
    pub max_joint_speed: Option<f64>,
}

/// Where the arm's controller would take the tool toward a pose, without moving.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PreviewPose {
    pub position: [f64; 3],
    /// None leaves the orientation to the solver.
    #[serde(default)]
    pub orientation: Option<PoseOrientation>,
}

/// Independent pose candidates for one arm; identifiers are supplied by the caller.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PosePreviewBatchRequest {
    pub arm: String,
    /// At least one independent candidate.
    #[schemars(length(min = 1))]
    pub poses: Vec<PosePreviewCandidate>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PosePreviewCandidate {
    /// A caller-generated UUID v4, unique within the batch.
    pub id: String,
    /// Optional human-readable context, echoed with the answer.
    #[serde(default)]
    pub label: Option<String>,
    pub pose: PreviewPose,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PosePreviewBatchResponse {
    /// One answer per candidate, in request order.
    pub results: Vec<PosePreviewCandidateResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PosePreviewCandidateResult {
    pub id: String,
    pub label: Option<String>,
    pub answer: PreviewAnswer,
}

/// How a command was answered: accepted with its records (one per arm for
/// `move_arms`), or refused, in which case nothing moved.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum CommandAnswer {
    #[schemars(title = "CommandAccepted")]
    Accepted { records: Vec<ArmRecord> },
    #[schemars(title = "CommandRefused")]
    Refused {
        reason: RejectionReason,
        detail: String,
    },
}

/// How a preview was answered.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum PreviewAnswer {
    #[schemars(title = "PreviewProjected")]
    Projected { projection: Box<PoseProjection> },
    #[schemars(title = "PreviewRefused")]
    Refused {
        reason: RejectionReason,
        detail: String,
    },
}

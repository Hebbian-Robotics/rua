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

use crate::{ArmRecord, PoseOrientation, PoseProjection, RejectionReason, Waypoint};

/// A normalized gripper target, from closed (0) to open (1).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "f64", into = "f64")]
pub struct GripperOpening(f64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidGripperOpening;

impl std::fmt::Display for InvalidGripperOpening {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("A gripper opening must be a finite number from 0 (closed) to 1 (open)")
    }
}

impl std::error::Error for InvalidGripperOpening {}

impl TryFrom<f64> for GripperOpening {
    type Error = InvalidGripperOpening;

    fn try_from(opening: f64) -> Result<Self, Self::Error> {
        if (0.0..=1.0).contains(&opening) {
            Ok(Self(opening))
        } else {
            Err(InvalidGripperOpening)
        }
    }
}

impl From<GripperOpening> for f64 {
    fn from(opening: GripperOpening) -> Self {
        opening.0
    }
}

impl JsonSchema for GripperOpening {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "GripperOpening".into()
    }

    fn inline_schema() -> bool {
        true
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({"title": "GripperOpening", "type": "number", "minimum": 0.0, "maximum": 1.0})
    }
}

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
    /// Return to the pose saved when the arm was enabled, at up to 2 rad/s.
    #[schemars(title = "RehomeCommand")]
    Rehome,
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
    /// Start streaming joint destinations while the arm is holding. Send
    /// later goals to the operation's goal endpoint. The stream ends when
    /// goals stop arriving, on stop, or on a trip.
    #[schemars(title = "StationStreamJointsCommand")]
    StreamJoints {
        joints: Vec<f64>,
        #[serde(default)]
        max_joint_speed: Option<f64>,
        /// The initial gripper target, 0 closed to 1 open, if the stream drives it.
        #[serde(default)]
        gripper_opening: Option<GripperOpening>,
    },
    /// Open or close the gripper, 0 closed to 1 open, whether the arm is holding
    /// or moving. It completes on arrival or on contact.
    #[schemars(title = "StationSetGripperCommand")]
    SetGripper {
        opening: GripperOpening,
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
pub struct ActionSequenceArmAction {
    #[serde(default)]
    pub movement: Option<ActionSequenceMovement>,
    #[serde(default)]
    #[schemars(range(min = 0.0, max = 1.0))]
    pub gripper_opening: Option<f64>,
    /// Speed for both resources in rad/s; overrides the movement speed when provided.
    #[serde(default)]
    pub max_joint_speed: Option<f64>,
}

/// One synchronized step; a single-arm action is a map with one entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ActionSequenceStep {
    pub arms: BTreeMap<String, ActionSequenceArmAction>,
}

impl ActionSequenceStep {
    #[must_use]
    pub fn single(arm: impl Into<String>, action: ActionSequenceArmAction) -> Self {
        Self {
            arms: BTreeMap::from([(arm.into(), action)]),
        }
    }
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

/// Submit one arm command with a client-generated operation identifier.
/// The station uses the identifier to recognize retries of the same submission.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CommandSubmissionRequest {
    /// A caller-generated UUID v4 identifying this operation.
    pub operation_id: String,
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
    /// A caller-generated UUID v4 identifying this operation.
    pub operation_id: String,
    pub station_instance_id: String,
    pub command: MoveArmsCommand,
    #[serde(default)]
    pub lease_token: Option<String>,
    #[serde(default)]
    pub expected_revisions: BTreeMap<String, u64>,
    #[serde(default)]
    pub trace_context: Option<crate::TraceContext>,
}

/// Cancel the operation identified by the route while its lease is current.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CommandCancellationRequest {
    pub station_instance_id: String,
    pub lease_token: String,
}

/// Update a live joint stream at its original speed.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StreamGoalUpdateRequest {
    pub station_instance_id: String,
    pub lease_token: String,
    /// Strictly increasing within this operation. Older goals cannot replace newer ones.
    pub sequence: u32,
    pub joints: Vec<f64>,
    #[serde(default)]
    pub gripper_opening: Option<GripperOpening>,
    #[serde(default)]
    pub expected_revision: Option<u64>,
}

/// An ordered set of steps to execute under the caller's lease.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ActionSequenceRequest {
    pub steps: Vec<ActionSequenceStep>,
    /// Revisions fence every participating arm throughout the sequence.
    #[serde(default)]
    pub expected_revisions: BTreeMap<String, u64>,
}

/// Start an action sequence under an explicit station lease.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ActionSequenceStartRequest {
    pub request_id: String,
    pub station_instance_id: String,
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
    Stopped { failure: crate::ExecutionFailure },
}

/// The station's retained action sequence state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ActionSequenceRecord {
    pub continuation: Option<crate::ProgramContinuation>,
    pub id: String,
    pub compilation_id: String,
    pub arms: Vec<String>,
    /// The submitted steps, including those not yet executed.
    pub requested_steps: Vec<ActionSequenceStep>,
    pub phase: ActionSequencePhase,
    /// Command records for steps that have finished or been interrupted.
    pub steps: Vec<Vec<ArmRecord>>,
}

/// Move several arms together: every target is planned before any arm moves,
/// one refusal refuses all, and when one arm ends without completing the others
/// stop and hold. No collision checking between the arms.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MoveArmsCommand {
    #[schemars(schema_with = "nonempty_arm_targets")]
    pub targets: BTreeMap<String, Waypoint>,
    #[serde(default)]
    pub max_joint_speed: Option<f64>,
}

fn nonempty_arm_targets(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
    let mut schema = <BTreeMap<String, Waypoint>>::json_schema(generator);
    schema.insert("minProperties".into(), 1.into());
    schema
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

/// Station-owned progress of one operation.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum OperationStatus {
    Pending,
    Active {
        records: Vec<ArmRecord>,
    },
    Cancelling {
        records: Vec<ArmRecord>,
    },
    Finished {
        records: Vec<ArmRecord>,
    },
    Refused {
        reason: RejectionReason,
        detail: String,
    },
    CancelledBeforeAdmission,
    OutcomeUnknown {
        detail: String,
    },
}

/// Select robot state and camera snapshots for one observation.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ObservationRequest {
    /// With no arm selection, include every arm.
    #[serde(default)]
    pub arms: Vec<String>,
    #[serde(default)]
    pub required_cameras: Vec<String>,
    #[serde(default)]
    pub optional_cameras: Vec<String>,
}

/// One captured JPEG with its source timestamp and retrieval interval.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ObservationFrame {
    pub jpeg_base64: String,
    /// Decimal nanoseconds, encoded as text to preserve precision across SDKs.
    pub captured_at_unix_ns: Option<String>,
    pub retrieved_between_unix_seconds: [f64; 2],
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ObservationAnswer {
    ObservationCaptured {
        state: Box<crate::StationState>,
        cameras: BTreeMap<String, ObservationFrame>,
        missing_optional_cameras: Vec<String>,
    },
    ObservationArmDisconnected {
        arm: String,
    },
    ObservationRevisionChanged {
        arm: String,
    },
    ObservationRequiredCameraUnavailable {
        camera: String,
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

#[cfg(test)]
mod gripper_opening_tests {
    use super::*;

    #[test]
    fn an_opening_accepts_the_closed_and_open_endpoints_as_wire_numbers() {
        for wire_value in ["0.0", "0.5", "1.0"] {
            let opening: GripperOpening = serde_json::from_str(wire_value).unwrap();
            assert_eq!(serde_json::to_string(&opening).unwrap(), wire_value);
        }
    }

    #[test]
    fn an_opening_rejects_values_outside_the_unit_interval() {
        for opening in [-0.1, 1.1, f64::NAN, f64::INFINITY] {
            assert!(GripperOpening::try_from(opening).is_err());
        }
        for wire_value in ["-0.1", "1.1"] {
            assert!(serde_json::from_str::<GripperOpening>(wire_value).is_err());
        }
    }
}

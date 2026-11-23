#![deny(clippy::wildcard_enum_match_arm)]
//! Shared station state and command-record contracts.

#![warn(clippy::missing_errors_doc, clippy::missing_panics_doc)]

mod controller_config;
pub use controller_config::*;
mod backoff;
pub mod server_sent_events;
pub use backoff::*;
mod lease;
pub use lease::*;
mod ik_outcomes;
pub use ik_outcomes::*;
mod station_status;
pub use station_status::*;
mod room_access;
pub use room_access::*;
mod sim_plant;
pub use sim_plant::*;
mod recording;
pub use recording::*;
mod http_api;
pub use http_api::*;
mod motion;
pub use motion::*;
mod robot_program;
pub use robot_program::*;
mod robot_tools;
pub use robot_tools::*;
mod protocol;
pub use protocol::*;
mod station_api;
pub use station_api::*;
mod telemetry;
pub use telemetry::*;
mod station_message;
pub use station_message::*;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A camera participant and recording-path name.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, JsonSchema)]
#[schemars(inline, transparent)]
pub struct CameraName(#[schemars(regex(pattern = r"^[a-z0-9_]{1,40}$"))] String);

impl<'de> Deserialize<'de> for CameraName {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::try_from(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

impl std::borrow::Borrow<str> for CameraName {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl TryFrom<String> for CameraName {
    type Error = InvalidCameraName;

    fn try_from(name: String) -> Result<Self, Self::Error> {
        if name.is_empty()
            || name.len() > 40
            || !name.bytes().all(|character| {
                character.is_ascii_lowercase() || character.is_ascii_digit() || character == b'_'
            })
        {
            return Err(InvalidCameraName(name));
        }
        Ok(Self(name))
    }
}

impl TryFrom<&str> for CameraName {
    type Error = InvalidCameraName;

    fn try_from(name: &str) -> Result<Self, Self::Error> {
        Self::try_from(name.to_owned())
    }
}

impl CameraName {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for CameraName {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Camera address used by the station. Simulator views carry a sim_ prefix and an opaque renderer name.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, JsonSchema)]
#[schemars(inline, transparent)]
pub struct SnapshotName(#[schemars(regex(pattern = SnapshotName::PATTERN))] String);

impl TryFrom<String> for SnapshotName {
    type Error = InvalidCameraName;

    fn try_from(name: String) -> Result<Self, Self::Error> {
        if name
            .strip_prefix("sim_")
            .is_some_and(|view| !view.is_empty())
        {
            return Ok(Self(name));
        }
        CameraName::try_from(name).map(|name| Self(name.0))
    }
}

impl<'de> Deserialize<'de> for SnapshotName {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::try_from(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

impl SnapshotName {
    pub const PATTERN: &'static str = r"^(?:[a-z0-9_]{1,40}|sim_[\s\S]+)$";
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SnapshotName {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidCameraName(String);

impl std::fmt::Display for InvalidCameraName {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "invalid camera name {:?}; use 1–40 lowercase letters, digits or underscores",
            self.0
        )
    }
}

impl std::error::Error for InvalidCameraName {}

#[cfg(test)]
mod camera_name_tests {
    use super::{CameraName, SnapshotName};

    #[test]
    fn simulator_views_round_trip_without_becoming_participant_names() {
        let renderer_view = "sim_Left wrist/close-up";
        let name: SnapshotName = serde_json::from_value(renderer_view.into()).unwrap();
        assert_eq!(name.as_str(), renderer_view);
        assert_eq!(serde_json::to_value(name).unwrap(), renderer_view);
        assert!(CameraName::try_from(renderer_view).is_err());
        assert!(serde_json::from_value::<SnapshotName>("../outside".into()).is_err());
    }

    #[test]
    fn camera_names_use_safe_ascii_and_have_bounded_length() {
        for name in ["a", "phone_front_2", "a".repeat(40).as_str()] {
            assert_eq!(CameraName::try_from(name).unwrap().as_str(), name);
        }
        for name in ["", "UPPER", "../outside", "café", "a".repeat(41).as_str()] {
            assert!(CameraName::try_from(name).is_err());
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SurfaceGuardConfig {
    pub surface_height_meters: f64,
    pub leeway_meters: f64,
}

/// Where the arm stood, as measured, when it settled at the end of a segment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Checkpoint {
    pub label: String,
    pub measured_joints: Vec<f64>,
    pub target_position: Option<[f64; 3]>,
    pub target_quaternion: Option<[f64; 4]>,
}

/// The tool-frame axis the gripper's jaws close along.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum JawClosingAxis {
    X,
    Y,
}

/// The world axis jaw yaw zero is projected from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum JawYawReference {
    WorldX,
    WorldY,
}

/// A tool orientation in grasp terms, rounded for people to read: where the jaws
/// point, how far that is from straight down, and the jaw yaw about it. With a
/// target, also how far the orientation is from it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct OrientationSummary {
    pub approach: [f64; 3],
    #[schemars(range(min = 0.0, max = 180.0))]
    pub degrees_from_down: f64,
    #[schemars(range(min = 0.0), extend("exclusiveMaximum" = 360.0))]
    pub jaw_yaw_degrees: f64,
    pub jaw_yaw_reference: JawYawReference,
    pub target: Option<TargetComparison>,
}

/// How an orientation compares with the one it was sent toward. Each jaw yaw
/// names its own reference: a change of reference near the switch is not a
/// physical error.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TargetComparison {
    #[schemars(range(min = 0.0), extend("exclusiveMaximum" = 360.0))]
    pub jaw_yaw_degrees: f64,
    pub jaw_yaw_reference: JawYawReference,
    #[schemars(range(min = 0.0, max = 180.0))]
    pub approach_error_degrees: f64,
    #[schemars(range(min = 0.0, max = 180.0))]
    pub orientation_error_degrees: f64,
}

/// The tool's pose at some joints, in the station's world frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PoseDescription {
    pub position: [f64; 3],
    pub quaternion: [f64; 4],
    pub orientation: OrientationSummary,
    /// Each joint's origin, then the tool's: the points a viewer draws the arm through.
    pub model_points: Vec<[f64; 3]>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ControllerKind {
    Ik,
    Policy,
}

/// Where the arm stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ArmMode {
    /// Torque off; only `enable` is accepted.
    Disabled,
    /// Torque on, holding between commands.
    Holding,
    /// Following a motion or a stream.
    Moving,
    /// Powered off after a hard fault; refuses everything until `clear_fault`.
    Faulted,
    /// The bus stopped answering: torque is off as far as the bus allowed, and
    /// the port is opened again every second. Refuses everything meanwhile.
    Disconnected,
    /// The bus answers again, set up as at startup, with torque off. Nothing
    /// resumes on its own: as when disabled, only `enable` is accepted.
    Reconnected,
}

impl ArmMode {
    /// Torque off, ready for `enable`.
    #[must_use]
    pub fn accepts_enable(self) -> bool {
        matches!(self, Self::Disabled | Self::Reconnected)
    }

    /// Refusing every command: faulted, or waiting for the bus to come back.
    #[must_use]
    pub fn refuses_everything(self) -> bool {
        matches!(self, Self::Faulted | Self::Disconnected)
    }
}

/// How a command ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CommandOutcome {
    Completed,
    // The client asked it to stop, or the arm was disabled.
    Stopped,
    // A safety limit tripped; the arm stopped and holds where it was.
    Tripped,
    // It ran past its budget; the arm holds where it is.
    TimedOut,
    // A planned motion came to rest short of its target, outside the settled
    // tolerance, and was no longer closing on it; `achieved` has each joint's error.
    StoppedShort,
    // A new command replaced it while it was only settling; `achieved` has the
    // error at that moment.
    Superseded,
    // A hard fault: the motors were powered off at once.
    Faulted,
    // Communication was lost; the arm may have continued toward its last goal.
    Interrupted,
    // A command failed. Its phase records whether it ran.
    Failed,
}

/// A move cut short where its straight joint path would first fold the arm into
/// itself or drive it down into the table: the plan's target is the last clear
/// sample before that, not what was asked for.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PathLimited {
    pub reason: String,
}

/// A pose move whose target no joints reach exactly, run to the closest pose the
/// solver found instead: the plan's target is that pose, not what was asked for.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PoseLimited {
    /// The waypoint the move stops at, counted from 0; later waypoints are left out.
    pub waypoint: usize,
    pub reason: IkFailure,
    /// From the closest pose's tool position to the requested one.
    #[schemars(range(min = 0.0))]
    pub position_shortfall_meters: f64,
    /// From the closest pose's orientation to the requested one.
    #[schemars(range(min = 0.0))]
    pub orientation_shortfall_radians: f64,
    /// The shortfall and the closest pose's tool position in millimetres, then what
    /// would be reachable exactly instead, in the terms a pose takes.
    pub advice: String,
}

/// Whether the arm ended where the command was aimed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TargetStatus {
    Reached,
    /// The arm reached an adjusted pose or path target.
    Limited,
    // Rest outside the settled tolerance, no longer closing on the target.
    StoppedShort,
    Partial,
    NotStarted,
}

/// Where the arm was, measured, when a command ended.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Achieved {
    pub sampled_at: f64,
    pub joints: Vec<f64>,
    /// The tool pose at `joints`; absent when the controller has no tool link.
    pub position: Option<[f64; 3]>,
    pub quaternion: Option<[f64; 4]>,
    #[schemars(range(min = 0.0))]
    pub joint_error_radians: Option<f64>,
    /// Each joint's absolute distance from the target joints.
    pub joint_errors_radians: Option<Vec<f64>>,
    #[schemars(range(min = 0.0))]
    pub position_error_meters: Option<f64>,
    #[schemars(range(min = 0.0))]
    pub orientation_error_radians: Option<f64>,
    /// The tool orientation in grasp terms, against the target's when there is one.
    pub orientation: Option<OrientationSummary>,
}

/// The caller's W3C trace context for a command, so a recorder can make the
/// command's span a child of the caller's span.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TraceContext {
    /// `00-<trace id>-<parent span id>-<flags>`.
    pub traceparent: String,
    #[serde(default)]
    pub tracestate: Option<String>,
}

/// The target pose or streamed goal in the station world frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CommandPose {
    pub position: [f64; 3],
    pub quaternion: [f64; 4],
}

/// State-changing commands with no position target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ControlAction {
    Enable,
    Disable,
    ApproveRecovery,
    ClearFault,
    Stop,
    Shutdown,
}

/// Data specific to the command the controller accepted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RecordedCommand {
    #[schemars(title = "RecordedMove")]
    Move {
        target_joints: Vec<f64>,
        target_position: Option<[f64; 3]>,
        target_quaternion: Option<[f64; 4]>,
        joint_speed: f64,
        path_limited: Option<PathLimited>,
        pose_limited: Option<PoseLimited>,
        group_id: Option<String>,
    },
    #[schemars(title = "RecordedPolicyReach")]
    PolicyReach { target_pose: CommandPose },
    #[schemars(title = "RecordedJointStream")]
    JointStream {
        goal_joints: Vec<f64>,
        joint_speed: f64,
    },
    #[schemars(title = "RecordedPoseStream")]
    PoseStream { goal_pose: CommandPose },
    #[schemars(title = "RecordedGripper")]
    Gripper {
        target_angle_radians: f64,
        joint_speed: f64,
        /// The joint stream that issued this jaw goal, if any.
        stream_command_id: Option<crate::CommandId>,
    },
    #[schemars(title = "RecordedControl")]
    Control { action: ControlAction },
}

/// Station identifier for one accepted command and its subsequent records.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(transparent)]
#[schemars(inline)]
pub struct CommandId(String);

impl CommandId {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::ops::Deref for CommandId {
    type Target = str;
    fn deref(&self) -> &str {
        self.as_str()
    }
}

impl std::borrow::Borrow<str> for CommandId {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl From<String> for CommandId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for CommandId {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl std::fmt::Display for CommandId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// One command, from acceptance to its end.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CommandRecord {
    pub command_id: crate::CommandId,
    /// A readable description, including the originating operation's name.
    pub label: String,
    pub accepted_unix_nano: UnixNanos,
    pub command: RecordedCommand,
    pub phase: CommandPhase,
    pub checkpoints: Vec<Checkpoint>,
    pub trace_context: Option<TraceContext>,
}

/// How a gripper command ended, in the jaw's own angle. The controller sees where the jaw
/// stopped, not what stopped it, so it cannot tell a held object from a blocked jaw.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GripperResult {
    /// The jaw arrived at the angle it was asked for.
    Reached { measured_angle_radians: f64 },
    /// The jaw stopped short of its target and stayed there: it closed on something,
    /// was blocked opening, or its target was clipped to the jaw's range.
    StoppedShort {
        measured_angle_radians: f64,
        target_angle_radians: f64,
    },
}

impl GripperResult {
    /// The human-readable form kept in [`CommandCompletion::detail`].
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::Reached {
                measured_angle_radians,
            } => format!("Reached {measured_angle_radians:.3} rad"),
            Self::StoppedShort {
                measured_angle_radians,
                target_angle_radians,
            } => format!(
                "Stopped at {measured_angle_radians:.3} rad short of {target_angle_radians:.3}: holding an object or blocked"
            ),
        }
    }
}

/// A command outcome established by the controller.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum KnownCommandOutcome {
    Completed,
    Stopped,
    Tripped,
    TimedOut,
    StoppedShort,
    Superseded,
    Faulted,
    Failed,
}

impl From<KnownCommandOutcome> for CommandOutcome {
    fn from(outcome: KnownCommandOutcome) -> Self {
        match outcome {
            KnownCommandOutcome::Completed => Self::Completed,
            KnownCommandOutcome::Stopped => Self::Stopped,
            KnownCommandOutcome::Tripped => Self::Tripped,
            KnownCommandOutcome::TimedOut => Self::TimedOut,
            KnownCommandOutcome::StoppedShort => Self::StoppedShort,
            KnownCommandOutcome::Superseded => Self::Superseded,
            KnownCommandOutcome::Faulted => Self::Faulted,
            KnownCommandOutcome::Failed => Self::Failed,
        }
    }
}

impl CommandOutcome {
    #[must_use]
    pub fn known(self) -> Option<KnownCommandOutcome> {
        Some(match self {
            Self::Completed => KnownCommandOutcome::Completed,
            Self::Stopped => KnownCommandOutcome::Stopped,
            Self::Tripped => KnownCommandOutcome::Tripped,
            Self::TimedOut => KnownCommandOutcome::TimedOut,
            Self::StoppedShort => KnownCommandOutcome::StoppedShort,
            Self::Superseded => KnownCommandOutcome::Superseded,
            Self::Faulted => KnownCommandOutcome::Faulted,
            Self::Failed => KnownCommandOutcome::Failed,
            Self::Interrupted => return None,
        })
    }
}

/// The command was stopped or failed before it started.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NotStartedOutcome {
    Stopped,
    Failed,
}

/// The outcome and timestamp for a command with a known end.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CommandCompletion {
    pub finished_unix_nano: UnixNanos,
    pub outcome: KnownCommandOutcome,
    pub detail: Option<String>,
    /// How a gripper command ended, when jaw feedback was available.
    pub gripper: Option<GripperResult>,
}

/// Evidence of how an accepted command ended.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CommandPhase {
    /// Accepted and awaiting a terminal result.
    #[schemars(title = "PendingCommand")]
    Pending,
    #[schemars(title = "CommandNotStarted")]
    NotStarted {
        finished_unix_nano: UnixNanos,
        outcome: NotStartedOutcome,
        detail: Option<String>,
    },
    #[schemars(title = "FinishedCommand")]
    Finished {
        completion: CommandCompletion,
        achieved: Option<Box<Achieved>>,
    },
    /// Control was interrupted and the final arm position could not be confirmed.
    #[schemars(title = "CommandOutcomeUnknown")]
    OutcomeUnknown {
        finished_unix_nano: UnixNanos,
        detail: Option<String>,
    },
}

impl CommandRecord {
    #[must_use]
    pub fn new(
        command_id: crate::CommandId,
        label: &str,
        accepted_unix_nano: UnixNanos,
        command: RecordedCommand,
    ) -> Self {
        Self {
            command_id,
            label: label.to_owned(),
            accepted_unix_nano,
            command,
            phase: CommandPhase::Pending,
            checkpoints: Vec::new(),
            trace_context: None,
        }
    }

    #[must_use]
    pub fn outcome(&self) -> Option<CommandOutcome> {
        match &self.phase {
            CommandPhase::Pending => None,
            CommandPhase::Finished { completion, .. } => Some(completion.outcome.into()),
            CommandPhase::NotStarted { outcome, .. } => Some(match outcome {
                NotStartedOutcome::Stopped => CommandOutcome::Stopped,
                NotStartedOutcome::Failed => CommandOutcome::Failed,
            }),
            CommandPhase::OutcomeUnknown { .. } => Some(CommandOutcome::Interrupted),
        }
    }

    #[must_use]
    pub fn finished_unix_nano(&self) -> Option<UnixNanos> {
        match &self.phase {
            CommandPhase::Pending => None,
            CommandPhase::NotStarted {
                finished_unix_nano, ..
            }
            | CommandPhase::OutcomeUnknown {
                finished_unix_nano, ..
            } => Some(*finished_unix_nano),
            CommandPhase::Finished { completion, .. } => Some(completion.finished_unix_nano),
        }
    }

    #[must_use]
    pub fn detail(&self) -> Option<&str> {
        match &self.phase {
            CommandPhase::Pending => None,
            CommandPhase::NotStarted { detail, .. }
            | CommandPhase::OutcomeUnknown { detail, .. } => detail.as_deref(),
            CommandPhase::Finished { completion, .. } => completion.detail.as_deref(),
        }
    }

    #[must_use]
    pub fn is_pending(&self) -> bool {
        matches!(self.phase, CommandPhase::Pending)
    }

    #[must_use]
    pub fn was_limited(&self) -> bool {
        matches!(
            &self.command,
            RecordedCommand::Move {
                path_limited: Some(_),
                ..
            } | RecordedCommand::Move {
                pose_limited: Some(_),
                ..
            }
        )
    }

    #[must_use]
    pub fn group_id(&self) -> Option<&str> {
        match &self.command {
            RecordedCommand::Move { group_id, .. } => group_id.as_deref(),
            RecordedCommand::PolicyReach { .. }
            | RecordedCommand::JointStream { .. }
            | RecordedCommand::PoseStream { .. }
            | RecordedCommand::Gripper { .. }
            | RecordedCommand::Control { .. } => None,
        }
    }

    #[must_use]
    pub fn target_joints(&self) -> Option<&[f64]> {
        match &self.command {
            RecordedCommand::Move { target_joints, .. } => Some(target_joints),
            RecordedCommand::PolicyReach { .. }
            | RecordedCommand::JointStream { .. }
            | RecordedCommand::PoseStream { .. }
            | RecordedCommand::Gripper { .. }
            | RecordedCommand::Control { .. } => None,
        }
    }

    #[must_use]
    pub fn target_position(&self) -> Option<[f64; 3]> {
        match &self.command {
            RecordedCommand::Move {
                target_position, ..
            } => *target_position,
            RecordedCommand::PolicyReach { target_pose } => Some(target_pose.position),
            RecordedCommand::JointStream { .. }
            | RecordedCommand::PoseStream { .. }
            | RecordedCommand::Gripper { .. }
            | RecordedCommand::Control { .. } => None,
        }
    }

    #[must_use]
    pub fn target_quaternion(&self) -> Option<[f64; 4]> {
        match &self.command {
            RecordedCommand::Move {
                target_quaternion, ..
            } => *target_quaternion,
            RecordedCommand::PolicyReach { target_pose } => Some(target_pose.quaternion),
            RecordedCommand::JointStream { .. }
            | RecordedCommand::PoseStream { .. }
            | RecordedCommand::Gripper { .. }
            | RecordedCommand::Control { .. } => None,
        }
    }

    #[must_use]
    pub fn achieved(&self) -> Option<&Achieved> {
        match &self.phase {
            CommandPhase::Finished { achieved, .. } => achieved.as_deref(),
            CommandPhase::Pending
            | CommandPhase::NotStarted { .. }
            | CommandPhase::OutcomeUnknown { .. } => None,
        }
    }

    #[must_use]
    pub fn target_status(&self) -> Option<TargetStatus> {
        self.target_joints()?;
        match self.phase {
            CommandPhase::Pending | CommandPhase::OutcomeUnknown { .. } => None,
            CommandPhase::NotStarted { .. } => Some(TargetStatus::NotStarted),
            CommandPhase::Finished { ref completion, .. } => Some(match completion.outcome {
                KnownCommandOutcome::Completed if self.was_limited() => TargetStatus::Limited,
                KnownCommandOutcome::Completed => TargetStatus::Reached,
                KnownCommandOutcome::StoppedShort => TargetStatus::StoppedShort,
                KnownCommandOutcome::Stopped
                | KnownCommandOutcome::Tripped
                | KnownCommandOutcome::TimedOut
                | KnownCommandOutcome::Superseded
                | KnownCommandOutcome::Faulted
                | KnownCommandOutcome::Failed => TargetStatus::Partial,
            }),
        }
    }
}

/// A command record and the arm it belongs to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ArmRecord {
    pub arm: String,
    pub record: CommandRecord,
}

/// Why automatic recovery stopped, including motors found without torque.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ArmRecoveryIssue {
    pub motors_without_torque: Vec<u8>,
    pub detail: String,
}

/// One arm as clients see it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ArmState {
    pub arm: String,
    /// The arm's URDF root in the station world frame. None means the world origin.
    #[serde(default)]
    pub base_pose: Option<BasePose>,
    pub mode: ArmMode,
    /// Station receipt time of the newest sample in Unix nanoseconds. Absent before the first sample.
    pub sampled_unix_nano: Option<UnixNanos>,
    pub measured_joints: Vec<f64>,
    /// Measured joint velocities in radians per second, in `measured_joints` order.
    pub measured_joint_velocities: Vec<f64>,
    /// Measured joint efforts in the hardware profile's effort units, in
    /// `measured_joints` order.
    pub measured_joint_efforts: Vec<f64>,
    /// Joint setpoint sent in the latest sample, if the arm was commanded.
    pub commanded_joints: Option<Vec<f64>>,
    /// The tool's pose at the measured joints, its orientation in grasp terms,
    /// and the joint origins a viewer sketches the arm through.
    pub tool: Option<PoseDescription>,
    /// Which controller turns commands into motion.
    pub controller: ControllerKind,
    /// The pose the arm was enabled at; shutdown returns there first.
    pub rest_pose: Option<Vec<f64>>,
    pub segment: Option<String>,
    pub active_command: Option<CommandRecord>,
    pub last_command: Option<CommandRecord>,
    pub fault: Option<String>,
    /// The protective check currently preventing new motion, if any.
    #[serde(default)]
    pub active_trip: Option<String>,
    pub recovery_issue: Option<ArmRecoveryIssue>,
    /// 0 closed to 1 open; none without a gripper.
    pub gripper_opening: Option<f64>,
    /// Conditions refusing motion on this arm until they clear.
    pub interlocks: Vec<Interlock>,
    /// Advanced when an interlock is raised or its details change. Commands may carry it as
    /// `expected_revision`.
    pub revision: u64,
}

/// The whole station, as `GET /state` and the event stream provide it.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct StationState {
    /// The profile name, absent for a bare controller configuration.
    pub robot: Option<String>,
    /// What a model is told about this robot: layout, frames, reach.
    pub description: Option<String>,
    pub joint_names: Vec<String>,
    /// None when the profile turns the table guard off.
    pub surface_guard: Option<SurfaceGuardConfig>,
    /// The tool-frame axis the jaws close along.
    pub jaw_closing_axis: JawClosingAxis,
    pub max_joint_speed: f64,
    pub arms: Vec<ArmState>,
    /// None while no one holds the lease: anyone may command.
    pub lease: Option<LeaseStatus>,
}

/// Nanoseconds since the Unix epoch now; a clock before the epoch reads 0 and
/// one past `u64` range saturates.
#[must_use]
pub fn unix_nanoseconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_nanos()).unwrap_or(u64::MAX)
        })
}

//! Robot program source, inspectable compilations, and execution records.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::{ArmRecord, GripperOpening, JointSpeed, PoseOrientation};

/// The final pose or joint configuration where this move settles.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ActionSequenceGoal {
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

/// An intermediate configuration along one continuous move.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ActionSequenceWaypoint {
    pub at: CompiledGoal,
    /// Start moving the gripper when passing this waypoint.
    #[schemars(extend("x-omit-when-none" = true))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gripper_opening: Option<GripperOpening>,
}

/// An arm action must name a goal or gripper opening. When both are present they start together.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged, deny_unknown_fields)]
pub enum ActionSequenceArmAction {
    #[schemars(title = "ActionSequenceMove")]
    Move {
        goal: ActionSequenceGoal,
        /// Pass through these intermediate configurations before settling at the goal.
        #[serde(default)]
        waypoints: Vec<ActionSequenceWaypoint>,
        #[serde(default)]
        #[schemars(range(min = 0.0, max = 1.0))]
        gripper_opening: Option<f64>,
        /// Speed for both resources in rad/s; overrides the goal speed when provided.
        #[serde(default)]
        max_joint_speed: Option<f64>,
        /// Joint acceleration cap in rad/s²; defaults to the arm profile.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[schemars(extend("exclusiveMinimum" = 0.0, "x-omit-when-none" = true))]
        max_joint_acceleration: Option<f64>,
        /// Joint jerk cap in rad/s³; defaults to the arm profile.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[schemars(extend("exclusiveMinimum" = 0.0, "x-omit-when-none" = true))]
        max_joint_jerk: Option<f64>,
    },
    #[schemars(title = "ActionSequenceGripper")]
    Gripper {
        #[schemars(range(min = 0.0, max = 1.0))]
        gripper_opening: f64,
        /// Gripper speed in rad/s.
        #[serde(default)]
        max_joint_speed: Option<f64>,
    },
}

impl ActionSequenceArmAction {
    #[must_use]
    pub fn goal(&self) -> Option<&ActionSequenceGoal> {
        match self {
            Self::Move { goal, .. } => Some(goal),
            Self::Gripper { .. } => None,
        }
    }

    #[must_use]
    pub fn gripper_opening(&self) -> Option<f64> {
        match self {
            Self::Move {
                gripper_opening, ..
            } => *gripper_opening,
            Self::Gripper {
                gripper_opening, ..
            } => Some(*gripper_opening),
        }
    }
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

/// An ordered set of steps to execute under the caller's lease.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ActionSequenceRequest {
    pub steps: Vec<ActionSequenceStep>,
    /// Revisions fence every participating arm throughout the sequence.
    #[serde(default)]
    pub expected_revisions: BTreeMap<String, u64>,
}

pub type CompiledStep = BTreeMap<String, CompiledOperation>;

/// A source identity remains stable while this step, arm and requested action are unchanged.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CompiledOperation {
    pub source_operation_id: String,
    pub action: CompiledArmAction,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CompiledGoal {
    Joints {
        joints: Vec<f64>,
    },
    Pose {
        position: [f64; 3],
        orientation: PoseOrientation,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CompiledArmAction {
    Move {
        goal: CompiledGoal,
        /// Pass through these intermediate configurations before settling at the goal.
        #[serde(default)]
        waypoints: Vec<ActionSequenceWaypoint>,
        gripper_opening: Option<GripperOpening>,
        max_joint_speed: Option<JointSpeed>,
        /// Joint acceleration cap in rad/s²; defaults to the arm profile.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[schemars(extend("exclusiveMinimum" = 0.0, "x-omit-when-none" = true))]
        max_joint_acceleration: Option<f64>,
        /// Joint jerk cap in rad/s³; defaults to the arm profile.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[schemars(extend("exclusiveMinimum" = 0.0, "x-omit-when-none" = true))]
        max_joint_jerk: Option<f64>,
    },
    Gripper {
        opening: GripperOpening,
        max_joint_speed: Option<JointSpeed>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CompiledProgram {
    pub steps: Vec<CompiledStep>,
    pub arms: Vec<String>,
    pub reused_operations: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CompilationTarget {
    pub configuration_identity: String,
    pub joint_count: usize,
    pub jaw_closing_axis: crate::JawClosingAxis,
    pub supports_joints: bool,
    pub supports_pose: bool,
    pub has_gripper: bool,
}

/// A structural diagnostic located in the submitted source.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CompilationDiagnostic {
    pub step: Option<usize>,
    pub arm: Option<String>,
    pub detail: String,
}

impl std::fmt::Display for CompilationDiagnostic {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(step) = self.step {
            write!(formatter, "Step {}", step + 1)?;
            if let Some(arm) = &self.arm {
                write!(formatter, ", arm {arm}")?;
            }
            write!(formatter, ": ")?;
        }
        formatter.write_str(&self.detail)
    }
}

/// Structural operations still require preparation from live measured state.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CompilationArtifact {
    pub schema_version: u32,
    pub compiler_version: String,
    pub structural_compilation_seconds: f64,
    pub compilation_id: String,
    pub station_instance_id: String,
    pub targets: BTreeMap<String, CompilationTarget>,
    pub source: ActionSequenceRequest,
    pub compiled: CompiledProgram,
    pub diagnostics: Vec<CompilationDiagnostic>,
}

/// Compilation does not acquire a lease or command hardware.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[schemars(rename = "CompilationOutcome")]
pub enum CompilationOutcome<Artifact = Box<CompilationArtifact>> {
    #[schemars(title = "ProgramCompiled")]
    Compiled { artifact: Artifact },
    #[schemars(title = "ProgramRejected")]
    Rejected {
        diagnostics: Vec<CompilationDiagnostic>,
    },
}

/// Explicitly select the unattempted suffix of a finished execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProgramContinuation {
    pub prior_execution_id: String,
    pub next_step: usize,
}

/// The boundary where an execution stopped; uncertain admission may already have acted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "stage", rename_all = "snake_case")]
pub enum ExecutionFailure {
    #[schemars(title = "PreparationFailure")]
    Preparation {
        step: usize,
        arm: String,
        reason: crate::RejectionReason,
        detail: String,
    },
    #[schemars(title = "AdmissionFailure")]
    Admission {
        step: usize,
        reason: crate::RejectionReason,
        detail: String,
    },
    #[schemars(title = "UncertainAdmission")]
    AdmissionUnknown { step: usize, detail: String },
    #[schemars(title = "MotionFailure")]
    Execution { step: usize, detail: String },
    #[schemars(title = "ExecutionCanceled")]
    Cancellation { step: usize, detail: String },
    #[schemars(title = "ExecutionAuthorityLost")]
    Authority { step: usize, detail: String },
    #[schemars(title = "RuntimeFailure")]
    Runtime { step: usize, detail: String },
}

impl ExecutionFailure {
    #[must_use]
    pub fn detail(&self) -> &str {
        match self {
            Self::Preparation { detail, .. }
            | Self::Admission { detail, .. }
            | Self::AdmissionUnknown { detail, .. }
            | Self::Execution { detail, .. }
            | Self::Cancellation { detail, .. }
            | Self::Authority { detail, .. }
            | Self::Runtime { detail, .. } => detail,
        }
    }
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
    Stopped { failure: ExecutionFailure },
}

/// The station's retained action sequence state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ActionSequenceRecord {
    pub continuation: Option<ProgramContinuation>,
    pub id: String,
    pub compilation_id: String,
    pub arms: Vec<String>,
    /// The submitted steps, including those not yet executed.
    pub requested_steps: Vec<ActionSequenceStep>,
    pub phase: ActionSequencePhase,
    /// Command records for steps that have finished or been interrupted.
    pub steps: Vec<Vec<ArmRecord>>,
}

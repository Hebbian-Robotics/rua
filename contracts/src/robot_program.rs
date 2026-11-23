//! Inspectable robot programs; execution accepts station-owned compilation identifiers.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::{ActionSequenceRequest, GripperOpening, PoseOrientation};

pub type CompiledStep = BTreeMap<String, CompiledOperation>;

/// A source identity remains stable while this step, arm and requested action are unchanged.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CompiledOperation {
    pub source_operation_id: String,
    pub action: CompiledArmAction,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CompiledMovement {
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
        movement: CompiledMovement,
        gripper_opening: Option<GripperOpening>,
        max_joint_speed: Option<f64>,
    },
    Gripper {
        opening: GripperOpening,
        max_joint_speed: Option<f64>,
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

/// Submit a station-owned compilation under current execution authority.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RunCompiledProgramRequest {
    pub request_id: String,
    pub station_instance_id: String,
    pub compilation_id: String,
    pub lease_token: String,
    #[serde(default)]
    pub continuation: Option<ProgramContinuation>,
    #[serde(default)]
    pub expected_revisions: BTreeMap<String, u64>,
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

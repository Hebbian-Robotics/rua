//! Who may command the arms (the lease), and what inhibits motion (interlocks).
//! See `docs/control-model.md`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The kind of controller holding the lease.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LeaseHolder {
    /// A model loop, started from the terminal or the agent server.
    Agent,
    /// A person driving the arms from the viewer's manual controls.
    Manual,
}

/// What happens to the lease when an arm is interrupted and needs an
/// operator's approval to recover.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OnInterruption {
    /// Keep the lease and wait for an operator's `approve_recovery`.
    #[default]
    WaitForApproval,
    /// Give the lease up.
    ReturnLease,
}

/// The requested holder and interruption policy for a station lease.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LeaseRequest {
    pub holder: LeaseHolder,
    pub label: String,
    #[serde(default)]
    pub required_cameras: Vec<crate::SnapshotName>,
    #[serde(default)]
    pub on_interruption: OnInterruption,
}

/// Acquire a station lease, optionally taking it from the current holder.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LeaseAcquireRequest {
    /// Caller-generated UUID v4, reused when retrying this acquisition.
    pub request_id: String,
    /// The station process observed before this acquisition was created.
    pub station_instance_id: String,
    pub request: LeaseRequest,
    #[serde(default)]
    pub take: bool,
}

/// Report a required camera's availability under the current lease.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LeaseCameraReport {
    pub lease_token: String,
    pub camera: crate::SnapshotName,
    pub available: bool,
}

/// The current lease, as everyone may see it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct LeaseStatus {
    pub holder: LeaseHolder,
    pub label: String,
    /// Cameras the holder cannot work without.
    pub required_cameras: Vec<crate::SnapshotName>,
    pub on_interruption: OnInterruption,
    /// The lease lapses this long after its grant or last explicit renewal.
    pub expires_in_seconds: f64,
}

/// Proof of the lease: send the token with every command.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct LeaseGrant {
    pub lease_token: String,
    pub lease_seconds: f64,
    pub status: LeaseStatus,
}

/// What a request about the lease answers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LeaseAnswer {
    /// The lease is yours.
    #[schemars(title = "LeaseGranted")]
    Granted(LeaseGrant),
    /// Who holds the lease, if anyone.
    #[schemars(title = "LeaseHeld")]
    Holder { status: Option<LeaseStatus> },
}

/// A condition on an arm that refuses motion until it clears. `stop`,
/// `disable`, `approve_recovery` and `clear_fault` are still accepted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Interlock {
    /// The bus stopped answering. Clears when the arm reconnects and holds.
    #[schemars(title = "ArmDisconnectedInterlock")]
    ArmDisconnected,
    /// The arm reconnected unpowered or failed its checks. Clears when an
    /// operator sends `approve_recovery` after supporting the arm.
    #[schemars(title = "RecoveryApprovalRequiredInterlock")]
    RecoveryApprovalRequired { motors: Vec<String> },
    /// A camera in the lease's `required_cameras` dropped. Clears when the
    /// holder reports a fresh frame, or when the lease ends.
    #[schemars(title = "RequiredCameraUnavailableInterlock")]
    RequiredCameraUnavailable { camera: String },
}

impl std::fmt::Display for Interlock {
    /// What inhibits motion and what clears it, for a refusal's `detail`.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ArmDisconnected => formatter
                .write_str("The arm is disconnected. Motion resumes when it reconnects and holds"),
            Self::RecoveryApprovalRequired { motors } => write!(
                formatter,
                "The arm needs recovery approval ({}). Support the arm, then approve recovery",
                motors.join(", ")
            ),
            Self::RequiredCameraUnavailable { camera } => write!(
                formatter,
                "Required camera {camera} is unavailable. Motion resumes when it gives a fresh frame"
            ),
        }
    }
}

/// A lease token, for routes that act only while it is current.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LeaseToken {
    pub lease_token: String,
}

/// Result of stopping an arm on behalf of a lease.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeasedStopOutcome {
    Submitted,
    LeaseLost,
    Unavailable,
}

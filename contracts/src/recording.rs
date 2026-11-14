use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Marker shared by the station and recorder for a controller interruption.
pub const CONTROLLER_INTERRUPTION_MARKER: &str = ".controller-interrupted";

/// Where the station's recorder is. Recording starts and stops only on request,
/// independent of power and the lease.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum RecordingStatus {
    Idle,
    Recording { name: String },
    Stopping { name: String },
    Finished { name: String },
    Failed { name: String, detail: String },
}

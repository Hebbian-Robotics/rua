use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Marker shared by the station and recorder for a controller interruption.
pub const CONTROLLER_INTERRUPTION_MARKER: &str = ".controller-interrupted";

/// Recorder exit code for a finalized archive with a gap in arm telemetry.
pub const RECORDER_CONTROLLER_INTERRUPTED_EXIT_CODE: u8 = 20;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RecordingWarning {
    ControllerInterrupted,
}

/// Where the station's recorder is. Recording starts and stops only on request,
/// independent of power and the lease.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum RecordingStatus {
    Idle,
    Recording {
        name: String,
    },
    Stopping {
        name: String,
    },
    Finished {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        warning: Option<RecordingWarning>,
    },
    Failed {
        name: String,
        detail: String,
    },
}

/// One recording directory. Its data, video included, is `recording.mcap`.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Recording {
    pub name: String,
    pub mcap_path: Option<String>,
    pub size_bytes: u64,
}
#[derive(Debug, Serialize, JsonSchema)]
pub struct Recordings {
    pub active: Option<String>,
    pub recordings: Vec<Recording>,
}

//! What the station reports to its viewer.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// What the viewer shows for driving the arm.
#[derive(Debug, Clone, Copy, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ViewerControlMode {
    /// A model loop drives; the viewer only watches.
    Llm,
    /// The viewer's pose controls drive a single SO-101.
    So101Manual,
}

/// `GET /viewer/api/control-mode`.
#[derive(Debug, Clone, Copy, Serialize, JsonSchema)]
pub struct ViewerControl {
    pub mode: ViewerControlMode,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum ControllerStatus {
    Unmanaged,
    Starting,
    Online,
    Stopping,
    Unavailable {
        reason: String,
        guard_released: bool,
        reports: Vec<String>,
    },
    WaitingForGuard {
        reason: String,
        reports: Vec<String>,
    },
}

#[derive(Deserialize, JsonSchema)]
pub struct ReconnectRequest {
    pub supported: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum MediaState {
    Running,
    /// It exited and will be started again.
    Restarting {
        last_exit: String,
    },
    /// It cannot be started, for instance because the binary is missing.
    Unavailable {
        reason: String,
    },
}

/// The current lifecycle of a configured camera.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum CameraStatus {
    Resolving,
    Starting,
    Capturing { publication: CameraPublication },
    Disconnected,
    Ambiguous,
    Stalled,
    Failed { detail: String },
}

impl CameraStatus {
    /// Explain why a fresh snapshot may be unavailable.
    #[must_use]
    pub fn snapshot_problem(&self) -> Option<String> {
        let detail = match self {
            Self::Resolving => "Looking up the camera",
            Self::Starting => "Waiting for the first local frame",
            Self::Disconnected => "USB camera disconnected (no matching device)",
            Self::Ambiguous => {
                "USB camera is ambiguous (multiple matching devices). Use rua camera-devices and pass --camera NAME=DEVICE"
            }
            Self::Stalled => "Local camera capture stalled. Retrying",
            Self::Failed { detail } => detail,
            Self::Capturing { publication } => match publication {
                CameraPublication::Connecting => "Local frames available. Connecting to LiveKit",
                CameraPublication::Reconnecting {
                    detail: Some(detail),
                } => {
                    return Some(format!(
                        "Local frames available. LiveKit reconnecting: {detail}"
                    ));
                }
                CameraPublication::Reconnecting { detail: None } => {
                    "Local frames available. LiveKit reconnecting"
                }
                CameraPublication::Disabled | CameraPublication::Publishing => return None,
            },
        };
        Some(detail.to_owned())
    }
}

/// Room publication is independent of the local capture lifecycle.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum CameraPublication {
    Disabled,
    Connecting,
    Publishing,
    Reconnecting { detail: Option<String> },
}

/// `GET /viewer/api/media-status`: the media service and its configured cameras.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct MediaStatusReport {
    pub service: MediaState,
    pub cameras: BTreeMap<String, CameraStatus>,
}

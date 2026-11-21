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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum MediaState {
    Running,
    /// Not asked for: `publish` without `LiveKit` credentials or USB cameras.
    NotConfigured,
    /// It exited and will be started again.
    Restarting {
        last_exit: String,
    },
    /// It cannot be started, for instance because the binary is missing.
    Unavailable {
        reason: String,
    },
}

/// Why a camera is not producing frames: the latest error a media program printed for it.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct CameraProblem {
    /// The program's message, such as `camera backend: Could not open device …`.
    pub detail: String,
    pub last_seen_unix_seconds: f64,
}

/// `GET /viewer/api/media-status`: each program's state and each camera's latest problem.
/// A problem stays until a newer one replaces it, so read it only for a camera that is
/// unavailable.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct MediaStatusReport {
    pub programs: BTreeMap<String, MediaState>,
    pub cameras: BTreeMap<String, CameraProblem>,
}

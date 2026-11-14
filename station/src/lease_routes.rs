//! The monitor that raises a required camera's interlock when the media server
//! stops listing the camera as available, and the types the station's HTTP
//! routes share. The lease holder reports cameras and requests lease-checked
//! stops through the station's HTTP API.

use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use axum::Json;
use axum::http::StatusCode;
use hebbian_robot_use_server::control_link::LinkHandle;

/// How often the monitor asks the media server which cameras are available.
const CAMERA_POLL_INTERVAL: Duration = Duration::from_secs(1);

/// The served arms, known once the station has set them up.
pub type ServedArms = Arc<OnceLock<BTreeMap<String, LinkHandle>>>;

/// A refused request, answered as `{"detail": ...}`.
pub type Refusal = (StatusCode, Json<serde_json::Value>);

pub fn refusal(status: StatusCode, detail: impl Into<String>) -> Refusal {
    (status, Json(serde_json::json!({ "detail": detail.into() })))
}

/// Mark the holder's required cameras unavailable whenever the media server
/// does not list them as available.
pub async fn monitor_required_cameras(link: LinkHandle, media_server_url: Option<String>) {
    #[derive(serde::Deserialize)]
    struct CameraStatus {
        name: String,
        available: bool,
    }
    let Ok(client) = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
    else {
        return;
    };
    let mut interval = tokio::time::interval(CAMERA_POLL_INTERVAL);
    loop {
        interval.tick().await;
        let Some(requirements) = link.camera_requirements() else {
            continue;
        };
        if requirements.cameras.is_empty() {
            continue;
        }
        let available = match &media_server_url {
            Some(base) => match client
                .get(format!("{}/cameras", base.trim_end_matches('/')))
                .send()
                .await
            {
                Ok(response) if response.status().is_success() => response
                    .json::<Vec<CameraStatus>>()
                    .await
                    .unwrap_or_default(),
                _ => Vec::new(),
            },
            None => Vec::new(),
        };
        for camera in &requirements.cameras {
            if !available
                .iter()
                .any(|status| &status.name == camera && status.available)
            {
                let _ = link.report_camera(&requirements.token, camera, false);
            }
        }
    }
}

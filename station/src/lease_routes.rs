use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use axum::Json;
use axum::http::StatusCode;
use hebbian_robot_use_server::control_link::LinkHandle;

const CAMERA_POLL_INTERVAL: Duration = Duration::from_secs(1);

pub type ServedArms = Arc<OnceLock<BTreeMap<String, LinkHandle>>>;

pub type Refusal = (StatusCode, Json<serde_json::Value>);

pub fn refusal(status: StatusCode, detail: impl Into<String>) -> Refusal {
    (status, Json(serde_json::json!({ "detail": detail.into() })))
}

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

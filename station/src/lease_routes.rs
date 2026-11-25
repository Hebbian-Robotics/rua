use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use axum::Json;
use axum::http::StatusCode;
use hebbian_contracts::CameraListing;
use hebbian_rua_core::control_link::LinkHandle;
use serde::{Deserialize, Serialize};

const CAMERA_POLL_INTERVAL: Duration = Duration::from_secs(1);

pub type ServedArms = Arc<OnceLock<BTreeMap<String, LinkHandle>>>;

#[derive(Deserialize, Serialize)]
pub struct RefusalBody {
    pub detail: String,
}

pub type Refusal = (StatusCode, Json<RefusalBody>);

pub fn refusal(status: StatusCode, detail: impl Into<String>) -> Refusal {
    (
        status,
        Json(RefusalBody {
            detail: detail.into(),
        }),
    )
}

pub async fn monitor_required_cameras(link: LinkHandle, camera_list_url: Option<String>) {
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
        let available = match camera_list_url.as_deref() {
            Some(url) => match client.get(url).send().await {
                Ok(response) if response.status().is_success() => response
                    .json::<Vec<CameraListing>>()
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

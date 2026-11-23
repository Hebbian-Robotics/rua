use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

use hebbian_contracts::{MediaState, MediaStatusReport};

#[derive(Debug, Clone)]
pub struct MediaStatus(Arc<Mutex<MediaState>>);

impl Default for MediaStatus {
    fn default() -> Self {
        Self(Arc::new(Mutex::new(MediaState::Unavailable {
            reason: "the station was started without a media service".to_owned(),
        })))
    }
}

impl MediaStatus {
    pub fn set(&self, state: MediaState) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = state;
    }

    #[must_use]
    pub fn report(&self) -> MediaStatusReport {
        MediaStatusReport {
            service: self
                .0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone(),
            cameras: BTreeMap::new(),
        }
    }

    pub(crate) async fn report_with_cameras(
        &self,
        proxy: Option<&crate::camera_proxy::CameraProxy>,
    ) -> MediaStatusReport {
        let mut report = self.report();
        if let Some(proxy) = proxy
            && report.service == MediaState::Running
        {
            match proxy.camera_status().await {
                Ok(cameras) => report.cameras = cameras,
                Err(error) => tracing::debug!(%error, "Camera status unavailable"),
            }
        }
        report
    }
}

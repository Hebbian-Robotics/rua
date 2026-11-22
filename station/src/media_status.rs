use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

use hebbian_contracts::{CameraProblem, MediaState, MediaStatusReport};

#[derive(Debug, Clone)]
pub struct MediaStatus(Arc<Mutex<MediaStatusReport>>);

impl Default for MediaStatus {
    fn default() -> Self {
        Self(Arc::new(Mutex::new(MediaStatusReport {
            service: MediaState::Unavailable {
                reason: "the station was started without a media service".to_owned(),
            },
            cameras: BTreeMap::new(),
        })))
    }
}

impl MediaStatus {
    pub fn set(&self, state: MediaState) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .service = state;
    }

    pub fn record_camera_problem(&self, camera: &str, detail: &str) {
        let last_seen_unix_seconds = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0.0, |elapsed| elapsed.as_secs_f64());
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .cameras
            .insert(
                camera.to_owned(),
                CameraProblem {
                    detail: detail.to_owned(),
                    last_seen_unix_seconds,
                },
            );
    }

    #[must_use]
    pub fn report(&self) -> MediaStatusReport {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

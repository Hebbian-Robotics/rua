use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::task::AbortHandle;
use tokio::time::Instant;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use hebbian_contracts::SequenceCameraHistory;

pub use hebbian_contracts::SequenceImageBoundary;

/// A historical JPEG with station retrieval timing, independent of robot state timing.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SequenceImage {
    pub camera: String,
    #[serde(serialize_with = "serialize_jpeg")]
    pub jpeg: Arc<[u8]>,
    pub retrieved_between_unix_seconds: [f64; 2],
    pub elapsed_seconds: f64,
    pub boundary: SequenceImageBoundary,
}

fn serialize_jpeg<S: serde::Serializer>(jpeg: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_bytes(jpeg)
}

/// Selected evidence from the latest model action sequence, bounded to eight images and 2 MiB.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SequenceImageHistory {
    pub call_id: String,
    pub images: Vec<SequenceImage>,
    pub missed_samples: usize,
    pub truncated: bool,
}

#[derive(Clone, Default)]
pub(crate) struct SequenceImages(Arc<Mutex<Option<LatestSequence>>>);

struct LatestSequence {
    id: String,
    call_id: String,
    cached: Option<CachedImages>,
}

struct CachedImages {
    expires_at: Instant,
    history: SequenceImageHistory,
    expiry: AbortHandle,
}

impl Drop for CachedImages {
    fn drop(&mut self) {
        self.expiry.abort();
    }
}

impl SequenceImages {
    pub(crate) fn select(&self, sequence: Option<(String, String)>) {
        *self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            sequence.map(|(id, call_id)| LatestSequence {
                id,
                call_id,
                cached: None,
            });
    }

    pub(crate) fn execution(&self) -> Option<String> {
        let latest = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Some(latest.as_ref()?.id.clone())
    }

    pub(crate) fn cache(&self, id: &str, history: SequenceCameraHistory) {
        let mut latest = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(latest) = latest.as_mut().filter(|latest| latest.id == id) else {
            return;
        };
        let images = history
            .images
            .into_iter()
            .map(|image| {
                Ok(SequenceImage {
                    camera: image.camera.to_string(),
                    jpeg: STANDARD.decode(image.frame.jpeg_base64)?.into(),
                    retrieved_between_unix_seconds: image.frame.retrieved_between_unix_seconds,
                    elapsed_seconds: image.elapsed_seconds,
                    boundary: image.boundary,
                })
            })
            .collect::<Result<Vec<_>, base64::DecodeError>>();
        if let Ok(images) = images {
            let expires_at = Instant::now()
                + Duration::from_secs_f64(history.expires_in_seconds.clamp(0.0, 300.0));
            let cache = Arc::downgrade(&self.0);
            let expiry = tokio::spawn(async move {
                tokio::time::sleep_until(expires_at).await;
                if let Some(cache) = cache.upgrade() {
                    let mut cache = cache
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    if let Some(latest) = cache.as_mut()
                        && latest
                            .cached
                            .as_ref()
                            .is_some_and(|entry| entry.expires_at <= Instant::now())
                    {
                        latest.cached = None;
                    }
                }
            })
            .abort_handle();
            latest.cached = Some(CachedImages {
                expires_at,
                expiry,
                history: SequenceImageHistory {
                    call_id: latest.call_id.clone(),
                    images,
                    missed_samples: history.missed_samples,
                    truncated: history.truncated,
                },
            });
        }
    }

    pub(crate) fn snapshot(&self) -> Option<SequenceImageHistory> {
        let mut latest = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let cached = &mut latest.as_mut()?.cached;
        if cached
            .as_ref()
            .is_some_and(|entry| entry.expires_at <= Instant::now())
        {
            *cached = None;
        }
        cached.as_ref().map(|entry| entry.history.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hebbian_contracts::{ObservationFrame, SequenceCameraImage, SnapshotName};

    #[tokio::test(start_paused = true)]
    async fn idle_history_releases_jpeg_at_its_deadline() {
        let images = SequenceImages::default();
        images.select(Some(("execution".to_owned(), "call".to_owned())));
        images.cache(
            "execution",
            SequenceCameraHistory {
                expires_in_seconds: 2.0,
                images: vec![SequenceCameraImage {
                    camera: SnapshotName::try_from("front".to_owned()).unwrap(),
                    frame: ObservationFrame {
                        jpeg_base64: "AQID".to_owned(),
                        captured_at_unix_ns: None,
                        retrieved_between_unix_seconds: [0.0, 1.0],
                    },
                    elapsed_seconds: 1.0,
                    boundary: SequenceImageBoundary::End,
                }],
                missed_samples: 0,
                truncated: false,
            },
        );
        let jpeg = Arc::downgrade(&images.snapshot().unwrap().images[0].jpeg);
        tokio::time::advance(Duration::from_secs(1)).await;
        assert!(jpeg.upgrade().is_some());
        tokio::time::advance(Duration::from_secs(1)).await;
        tokio::task::yield_now().await;
        assert!(jpeg.upgrade().is_none());
    }
}

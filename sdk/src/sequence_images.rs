use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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
    cached: Option<(Instant, SequenceImageHistory)>,
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
            latest.cached = Some((
                Instant::now()
                    + Duration::from_secs_f64(history.expires_in_seconds.clamp(0.0, 300.0)),
                SequenceImageHistory {
                    call_id: latest.call_id.clone(),
                    images,
                    missed_samples: history.missed_samples,
                    truncated: history.truncated,
                },
            ));
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
            .is_some_and(|(expires_at, _)| *expires_at <= Instant::now())
        {
            *cached = None;
        }
        cached.as_ref().map(|(_, history)| history.clone())
    }
}

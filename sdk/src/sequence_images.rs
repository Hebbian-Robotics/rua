use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use hebbian_contracts::ActionSequencePhase;
use tokio::sync::{oneshot, watch};
use tokio::task::JoinHandle;

use crate::Station;

const MAXIMUM_BYTES: usize = 16 * 1024 * 1024;
const MAXIMUM_IMAGES: usize = 64;
const MAXIMUM_MODEL_BYTES: usize = 2 * 1024 * 1024;
const MAXIMUM_MODEL_IMAGES: usize = 8;
const RETENTION: Duration = Duration::from_secs(300);

/// Why a best-effort camera sample was requested.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SequenceImageBoundary {
    Start,
    Motion,
    Step,
    End,
}

impl std::fmt::Display for SequenceImageBoundary {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Start => "start",
            Self::Motion => "motion",
            Self::Step => "step",
            Self::End => "end",
        })
    }
}

/// A historical JPEG with client retrieval timing, independent of robot state timing.
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

struct RetainedImages {
    history: SequenceImageHistory,
    bytes: usize,
    session: String,
    updated_at: Instant,
    expiry: Option<JoinHandle<()>>,
}

impl Drop for RetainedImages {
    fn drop(&mut self) {
        if let Some(expiry) = self.expiry.take() {
            expiry.abort();
        }
    }
}

#[derive(Clone, Default)]
pub(crate) struct SequenceImages(Arc<Mutex<Option<RetainedImages>>>);

impl SequenceImages {
    fn clear(&self) {
        *self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }

    fn begin(&self, call_id: String) -> String {
        let session = uuid::Uuid::new_v4().to_string();
        *self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(RetainedImages {
            history: SequenceImageHistory {
                call_id,
                images: Vec::new(),
                missed_samples: 0,
                truncated: false,
            },
            bytes: 0,
            session: session.clone(),
            updated_at: Instant::now(),
            expiry: None,
        });
        session
    }

    fn add(&self, session: &str, image: Option<SequenceImage>) {
        let mut retained = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(retained) = retained
            .as_mut()
            .filter(|retained| retained.session == session)
        else {
            return;
        };
        retained.updated_at = Instant::now();
        let Some(image) = image else {
            retained.history.missed_samples += 1;
            return;
        };
        if image.jpeg.len() > MAXIMUM_MODEL_BYTES {
            retained.history.missed_samples += 1;
            return;
        }
        retained.bytes += image.jpeg.len();
        retained.history.images.push(image);
        while retained.history.images.len() > MAXIMUM_IMAGES || retained.bytes > MAXIMUM_BYTES {
            retained.bytes -= retained.history.images.remove(1).jpeg.len();
            retained.history.truncated = true;
        }
    }

    fn finish(&self, session: &str) {
        let weak = Arc::downgrade(&self.0);
        let mut retained = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(retained) = retained
            .as_mut()
            .filter(|retained| retained.session == session)
        {
            retained.updated_at = Instant::now();
            retained.expiry = Some(tokio::spawn(expire_history(weak, session.to_owned())));
        }
    }

    pub(crate) fn snapshot(&self) -> Option<SequenceImageHistory> {
        let mut retained = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if retained
            .as_ref()
            .is_some_and(|retained| retained.updated_at.elapsed() >= RETENTION)
        {
            *retained = None;
        }
        let history = &retained.as_ref()?.history;
        let count = history.images.len().min(MAXIMUM_MODEL_IMAGES);
        let mut indices = (0..count)
            .map(|index| {
                (index * history.images.len().saturating_sub(1) + count.saturating_sub(1) / 2)
                    / count.saturating_sub(1).max(1)
            })
            .collect::<Vec<_>>();
        indices.reverse();
        let mut images = Vec::new();
        let mut bytes = 0;
        for index in indices {
            let image = &history.images[index];
            if bytes + image.jpeg.len() <= MAXIMUM_MODEL_BYTES {
                bytes += image.jpeg.len();
                images.push(image.clone());
            }
        }
        images.reverse();
        Some(SequenceImageHistory {
            images,
            ..history.clone()
        })
    }
}

async fn expire_history(retained: Weak<Mutex<Option<RetainedImages>>>, session: String) {
    tokio::time::sleep(RETENTION).await;
    if let Some(retained) = retained.upgrade() {
        let mut retained = retained
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if retained
            .as_ref()
            .is_some_and(|retained| retained.session == session)
        {
            *retained = None;
        }
    }
}

pub(crate) struct SequenceCapture {
    worker: JoinHandle<()>,
    stop: Option<oneshot::Sender<()>>,
    sequence_id: watch::Sender<Option<String>>,
    store: SequenceImages,
    session: String,
}

impl SequenceCapture {
    pub(crate) fn start(
        store: SequenceImages,
        station: Station,
        cameras: Vec<String>,
        call_id: String,
    ) -> Option<Self> {
        if cameras.is_empty() {
            store.clear();
            return None;
        }
        let session = store.begin(call_id);
        let (stop, stopping) = oneshot::channel();
        let (sequence_id, current_sequence_id) = watch::channel(None);
        let worker = tokio::spawn(sample_sequence(
            store.clone(),
            session.clone(),
            station,
            cameras,
            stopping,
            current_sequence_id,
        ));
        Some(Self {
            worker,
            stop: Some(stop),
            sequence_id,
            store,
            session,
        })
    }

    pub(crate) fn set_sequence_id(&self, sequence_id: String) {
        self.sequence_id.send_replace(Some(sequence_id));
    }

    pub(crate) async fn finish(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        let _ = (&mut self.worker).await;
    }
}

impl Drop for SequenceCapture {
    fn drop(&mut self) {
        self.worker.abort();
        self.store.finish(&self.session);
    }
}

async fn sample_sequence(
    store: SequenceImages,
    session: String,
    station: Station,
    cameras: Vec<String>,
    mut stopping: oneshot::Receiver<()>,
    sequence_id: watch::Receiver<Option<String>>,
) {
    let started_at = Instant::now();
    let mut updates = station.action_sequence_updates();
    let mut interval = tokio::time::interval(Duration::from_millis(250));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    interval.tick().await;
    let mut last_step = None;
    let mut camera_offset = 0;
    let mut boundary = SequenceImageBoundary::Start;
    loop {
        // CONSTRAINT: Camera I/O has a separate task and deadline; motion never awaits it.
        let capture = tokio::time::timeout(Duration::from_millis(500), async {
            for index in 0..cameras.len() {
                let camera = &cameras[(index + camera_offset) % cameras.len()];
                let image = read_image(&station, camera, started_at, boundary).await;
                store.add(&session, image);
            }
        });
        tokio::select! {
            result = capture => if result.is_err() { store.add(&session, None); },
            _ = &mut stopping, if boundary != SequenceImageBoundary::End => {
                boundary = SequenceImageBoundary::End;
                continue;
            }
        }
        camera_offset = (camera_offset + 1) % cameras.len();
        if boundary == SequenceImageBoundary::End {
            break;
        }
        boundary = loop {
            tokio::select! {
                _ = &mut stopping => break SequenceImageBoundary::End,
                _ = interval.tick() => break SequenceImageBoundary::Motion,
                changed = updates.changed() => {
                    if changed.is_err() { break SequenceImageBoundary::Motion; }
                    let current = updates.borrow_and_update();
                    if let Some(current) = current.as_ref()
                        && Some(&current.id) == sequence_id.borrow().as_ref()
                        && let ActionSequencePhase::Running { step } = current.phase
                        && last_step != Some(step)
                    {
                        last_step = Some(step);
                        break SequenceImageBoundary::Step;
                    }
                }
            }
        };
    }
}

async fn read_image(
    station: &Station,
    camera: &str,
    started_at: Instant,
    boundary: SequenceImageBoundary,
) -> Option<SequenceImage> {
    let before = unix_seconds();
    let mut response = station.get_camera(camera).await.ok()?;
    let mut jpeg = Vec::new();
    while let Some(chunk) = response.chunk().await.ok()? {
        if jpeg.len() + chunk.len() > MAXIMUM_MODEL_BYTES {
            return None;
        }
        jpeg.extend_from_slice(&chunk);
    }
    Some(SequenceImage {
        camera: camera.to_owned(),
        jpeg: jpeg.into(),
        retrieved_between_unix_seconds: [before, unix_seconds()],
        elapsed_seconds: started_at.elapsed().as_secs_f64(),
        boundary,
    })
}

fn unix_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn retention_and_model_budgets_keep_recent_evidence() {
        for jpeg_bytes in [1, 1024 * 1024] {
            let store = SequenceImages::default();
            let session = store.begin("move".into());
            for index in 0..100_u32 {
                store.add(
                    &session,
                    Some(SequenceImage {
                        camera: "front".into(),
                        jpeg: vec![0; jpeg_bytes].into(),
                        retrieved_between_unix_seconds: [f64::from(index), f64::from(index) + 0.1],
                        elapsed_seconds: f64::from(index),
                        boundary: if index == 99 {
                            SequenceImageBoundary::End
                        } else {
                            SequenceImageBoundary::Motion
                        },
                    }),
                );
            }
            {
                let retained = store.0.lock().unwrap();
                let retained = retained.as_ref().unwrap();
                assert!(retained.history.images.len() <= 64);
                assert!(retained.bytes <= 16 * 1024 * 1024);
            }
            let selected = store.snapshot().unwrap();
            assert!(selected.truncated);
            assert!(selected.images.len() <= 8);
            assert!(
                selected
                    .images
                    .iter()
                    .map(|image| image.jpeg.len())
                    .sum::<usize>()
                    <= 2 * 1024 * 1024
            );
            assert_eq!(
                selected.images.last().unwrap().boundary,
                SequenceImageBoundary::End
            );
            let _ = store.begin("next".into());
            store.add(&session, Some(selected.images[0].clone()));
            assert!(store.snapshot().unwrap().images.is_empty());
        }
    }
}

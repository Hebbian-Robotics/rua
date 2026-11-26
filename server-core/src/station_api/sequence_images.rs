use std::collections::{BTreeSet, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use hebbian_contracts::{
    ActionSequencePhase, ActionSequenceRecord, ObservationFrame, SequenceCameraHistory,
    SequenceCameraImage, SequenceImageBoundary, SnapshotName,
};
use tokio::sync::watch;
use tokio::time::Instant;

const MAXIMUM_BYTES: usize = 16 * 1024 * 1024;
const MAXIMUM_IMAGES: usize = 64;
const MAXIMUM_MODEL_BYTES: usize = 2 * 1024 * 1024;
const MAXIMUM_MODEL_IMAGES: usize = 8;
const MAXIMUM_HISTORIES: usize = 8;
const MAXIMUM_CAMERAS: usize = 16;
const RETENTION: Duration = Duration::from_secs(300);
const SAMPLE_TIMEOUT: Duration = Duration::from_millis(500);

pub type SequenceCameraSource = Arc<
    dyn Fn(SnapshotName, usize) -> Pin<Box<dyn Future<Output = Option<ObservationFrame>> + Send>>
        + Send
        + Sync,
>;

struct RetainedHistory {
    expires_at: Instant,
    history: SequenceCameraHistory,
}

type CaptureOutcome = watch::Receiver<Option<Arc<RetainedHistory>>>;

#[derive(Clone, Default)]
pub(super) struct SequenceImages(Arc<Mutex<VecDeque<(String, CaptureOutcome)>>>);

impl SequenceImages {
    pub(super) fn start(
        &self,
        id: String,
        progress: watch::Receiver<ActionSequenceRecord>,
        cameras: Vec<SnapshotName>,
        source: Option<SequenceCameraSource>,
    ) {
        let (finish, outcome) = watch::channel(None);
        {
            let mut entries = self
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if entries.len() == MAXIMUM_HISTORIES {
                let Some(index) = entries.iter().position(|(_, outcome)| {
                    outcome.has_changed().is_err() || outcome.borrow().is_some()
                }) else {
                    return;
                };
                entries.remove(index);
            }
            entries.push_back((id, outcome));
        }
        tokio::spawn(async move {
            let history = sample_sequence(progress, cameras, source).await;
            finish.send_replace(Some(Arc::new(RetainedHistory {
                expires_at: Instant::now() + RETENTION,
                history,
            })));
        });
    }

    pub(super) fn start_cleanup(&self, runtime: &tokio::runtime::Handle) {
        let retained = Arc::downgrade(&self.0);
        runtime.spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(30));
            loop {
                interval.tick().await;
                let Some(retained) = retained.upgrade() else {
                    return;
                };
                retained
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .retain(|(_, outcome)| {
                        outcome
                            .borrow()
                            .as_ref()
                            .is_none_or(|result| result.expires_at > Instant::now())
                    });
            }
        });
    }

    pub(super) async fn read(&self, id: &str) -> Option<SequenceCameraHistory> {
        let mut outcome = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .find(|(execution, _)| execution == id)?
            .1
            .clone();
        // WHY: The final camera batch may still be completing when an SDK sees the terminal record.
        let value = tokio::time::timeout(SAMPLE_TIMEOUT * 3, outcome.wait_for(Option::is_some))
            .await
            .ok()?
            .ok()?;
        let retained = value.as_ref()?;
        let expires_in_seconds = retained
            .expires_at
            .checked_duration_since(Instant::now())?
            .as_secs_f64();
        Some(SequenceCameraHistory {
            expires_in_seconds,
            ..retained.history.clone()
        })
    }
}

struct ImageBuffer {
    history: SequenceCameraHistory,
    bytes: usize,
}

impl ImageBuffer {
    fn add(&mut self, image: SequenceCameraImage) {
        let size = jpeg_bytes(&image);
        if size > MAXIMUM_MODEL_BYTES {
            self.history.missed_samples += 1;
            return;
        }
        self.bytes += size;
        self.history.images.push(image);
        while self.history.images.len() > MAXIMUM_IMAGES || self.bytes > MAXIMUM_BYTES {
            self.bytes -= jpeg_bytes(&self.history.images.remove(1));
            self.history.truncated = true;
        }
    }

    fn finish(mut self) -> SequenceCameraHistory {
        let images = std::mem::take(&mut self.history.images);
        let count = images.len().min(MAXIMUM_MODEL_IMAGES);
        let range = count.saturating_sub(1);
        let mut bytes = 0;
        for index in (0..count).rev() {
            let image = &images[(index * (images.len() - 1) + range / 2) / range.max(1)];
            if bytes + jpeg_bytes(image) <= MAXIMUM_MODEL_BYTES {
                bytes += jpeg_bytes(image);
                self.history.images.push(image.clone());
            }
        }
        self.history.images.reverse();
        self.history.truncated |= self.history.images.len() < images.len();
        self.history
    }
}

fn jpeg_bytes(image: &SequenceCameraImage) -> usize {
    let encoded = &image.frame.jpeg_base64;
    encoded.len() / 4 * 3
        - (encoded.len() - encoded.trim_end_matches('=').len()).min(encoded.len() / 4 * 3)
}

async fn sample_sequence(
    mut progress: watch::Receiver<ActionSequenceRecord>,
    mut cameras: Vec<SnapshotName>,
    source: Option<SequenceCameraSource>,
) -> SequenceCameraHistory {
    let mut unique = BTreeSet::new();
    cameras.retain(|camera| unique.insert(camera.clone()));
    let ignored = cameras.len().saturating_sub(MAXIMUM_CAMERAS);
    cameras.truncate(MAXIMUM_CAMERAS);
    let history = SequenceCameraHistory {
        expires_in_seconds: 0.0,
        images: Vec::new(),
        missed_samples: ignored,
        truncated: ignored > 0,
    };
    let Some(source) = source else {
        return SequenceCameraHistory {
            missed_samples: ignored + cameras.len(),
            ..history
        };
    };
    let mut buffer = ImageBuffer { history, bytes: 0 };
    let started = Instant::now();
    let mut interval = tokio::time::interval_at(
        started + Duration::from_millis(250),
        Duration::from_millis(250),
    );
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut boundary = SequenceImageBoundary::Start;
    let mut last_step = running_step(&progress.borrow());
    loop {
        let deadline = Instant::now() + SAMPLE_TIMEOUT;
        for camera in &cameras {
            if Instant::now() >= deadline {
                buffer.history.missed_samples += 1;
                continue;
            }
            let frame =
                tokio::time::timeout_at(deadline, source(camera.clone(), MAXIMUM_MODEL_BYTES))
                    .await
                    .ok()
                    .flatten();
            match frame {
                Some(frame) => buffer.add(SequenceCameraImage {
                    camera: camera.clone(),
                    frame,
                    elapsed_seconds: started.elapsed().as_secs_f64(),
                    boundary,
                }),
                None => buffer.history.missed_samples += 1,
            }
        }
        if boundary == SequenceImageBoundary::End {
            return buffer.finish();
        }
        cameras.rotate_left(1);
        boundary = loop {
            let step = running_step(&progress.borrow_and_update());
            if step.is_none() {
                break SequenceImageBoundary::End;
            }
            if step != last_step {
                last_step = step;
                break SequenceImageBoundary::Step;
            }
            tokio::select! {
                changed = progress.changed() => if changed.is_err() { break SequenceImageBoundary::End; },
                _ = interval.tick() => break SequenceImageBoundary::Motion,
            }
        };
    }
}

fn running_step(record: &ActionSequenceRecord) -> Option<usize> {
    match record.phase {
        ActionSequencePhase::Running { step } => Some(step),
        ActionSequencePhase::Completed | ActionSequencePhase::Stopped { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_budgets_preserve_recent_evidence() {
        for encoded_bytes in [4, 1024 * 1024] {
            let mut buffer = ImageBuffer {
                bytes: 0,
                history: SequenceCameraHistory {
                    expires_in_seconds: 0.0,
                    images: Vec::new(),
                    missed_samples: 0,
                    truncated: false,
                },
            };
            let camera = SnapshotName::try_from("front".to_owned()).unwrap();
            for index in 0..100 {
                buffer.add(SequenceCameraImage {
                    camera: camera.clone(),
                    frame: ObservationFrame {
                        jpeg_base64: "A".repeat(encoded_bytes),
                        captured_at_unix_ns: None,
                        retrieved_between_unix_seconds: [0.0, 0.0],
                    },
                    elapsed_seconds: f64::from(index),
                    boundary: if index == 99 {
                        SequenceImageBoundary::End
                    } else {
                        SequenceImageBoundary::Motion
                    },
                });
            }
            assert!(buffer.history.images.len() <= 64);
            assert!(buffer.bytes <= 16 * 1024 * 1024);
            let selected = buffer.finish();
            assert!(selected.truncated);
            assert!(selected.images.len() <= 8);
            assert!(selected.images.iter().map(jpeg_bytes).sum::<usize>() <= 2 * 1024 * 1024);
            assert_eq!(
                selected.images.last().unwrap().boundary,
                SequenceImageBoundary::End
            );
        }
    }
}

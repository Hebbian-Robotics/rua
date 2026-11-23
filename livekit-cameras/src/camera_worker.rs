use std::{sync::Arc, time::Duration};

use hebbian_contracts::{Backoff, CameraPublication, CameraStatus};
use livekit_camera_publisher::{
    capture::CaptureWorker,
    publisher::{self, PublicationEvent},
};
use station_access::{CameraLifetime, Credentials};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::{
    camera_lifecycle::{self, ActiveCapture, CaptureDecision, CaptureEvent, PublicationDecision},
    config::{CameraConfiguration, CameraInput},
    error::PublisherError,
    snapshots::Snapshots,
};

pub(super) struct RoomPublication {
    pub room: String,
    pub credentials: Credentials,
}

pub(super) async fn supervise(
    camera: CameraInput,
    room: Option<Arc<RoomPublication>>,
    snapshots: Snapshots,
    stopped: CancellationToken,
) {
    let name = camera.name();
    if room.is_none() {
        snapshots.register_local_frames(name);
    }
    let mut backoff = Backoff::new(Duration::from_secs(3));
    while !stopped.is_cancelled() {
        let lookup = tokio::select! {
            () = stopped.cancelled() => break,
            lookup = camera.resolve() => lookup,
        };
        let unavailable = match camera_lifecycle::after_lookup(lookup) {
            CaptureDecision::Open(camera) => {
                snapshots.set_camera_status(name, CameraStatus::Starting);
                match CaptureWorker::start(&camera) {
                    Ok(worker) => {
                        if run_capture(&camera, &worker, room.as_deref(), &snapshots, &stopped)
                            .await
                        {
                            backoff.reset();
                        }
                        camera_lifecycle::after_capture(worker.stop().await)
                    }
                    Err(error) => camera_lifecycle::capture_failed(error),
                }
            }
            CaptureDecision::Retry(unavailable) => unavailable,
        };
        if stopped.is_cancelled() {
            break;
        }
        tracing::debug!(camera = name, status = ?unavailable, "Retrying camera capture");
        snapshots.set_camera_status(name, unavailable);
        tokio::select! {
            () = stopped.cancelled() => break,
            () = tokio::time::sleep(backoff.next_delay()) => {},
        }
    }
}

async fn run_capture(
    camera: &CameraConfiguration,
    worker: &CaptureWorker,
    room: Option<&RoomPublication>,
    snapshots: &Snapshots,
    stopped: &CancellationToken,
) -> bool {
    let mut state = ActiveCapture::WaitingForFrame(if room.is_some() {
        CameraPublication::Connecting
    } else {
        CameraPublication::Disabled
    });
    let mut frames = worker.frames.clone();
    let publication_stopped = stopped.child_token();
    let (publication_updates, mut updates) = mpsc::unbounded_channel();
    // INVARIANT: Each worker owns its publication future and event receiver. Both are dropped
    // before the next lookup, so an earlier session cannot update its successor.
    let publication = async {
        if let Some(room) = room {
            publish(
                camera,
                worker,
                room,
                publication_updates,
                &publication_stopped,
            )
            .await;
        } else {
            publication_stopped.cancelled().await;
        }
    };
    tokio::pin!(publication);
    loop {
        let event = tokio::select! {
            () = worker.finished() => {
                publication_stopped.cancel();
                publication.await;
                break;
            }
            () = stopped.cancelled() => {
                publication.await;
                break;
            }
            () = &mut publication => break,
            Some(status) = updates.recv() => CaptureEvent::PublicationChanged(status),
            changed = frames.changed() => {
                if changed.is_err() {
                    publication_stopped.cancel();
                    publication.await;
                    break;
                }
                let Some(frame) = frames.borrow_and_update().clone() else {
                    continue;
                };
                if room.is_none() {
                    snapshots.store_local_frame(camera.name(), &frame);
                }
                if matches!(state, ActiveCapture::ReceivingFrames(_)) {
                    continue;
                }
                CaptureEvent::FrameReceived
            }
        };
        state = state.transition(event);
        snapshots.set_camera_status(camera.name(), state.status());
    }
    matches!(state, ActiveCapture::ReceivingFrames(_))
}

async fn publish(
    camera: &CameraConfiguration,
    worker: &CaptureWorker,
    room: &RoomPublication,
    updates: mpsc::UnboundedSender<CameraPublication>,
    stopped: &CancellationToken,
) {
    let mut backoff = Backoff::new(Duration::from_secs(3));
    while !stopped.is_cancelled() {
        let started = std::time::Instant::now();
        let result = async {
            let access = room.credentials.camera_access(
                &room.room,
                camera.name(),
                CameraLifetime::Station,
            )?;
            publisher::publish_camera_frames(
                camera,
                &access.server_url,
                &access.token,
                worker.frames.clone(),
                stopped.clone(),
                |event| {
                    let _ = updates.send(match event {
                        PublicationEvent::Publishing => CameraPublication::Publishing,
                        PublicationEvent::Reconnecting => {
                            CameraPublication::Reconnecting { detail: None }
                        }
                    });
                },
            )
            .await?;
            Ok::<(), PublisherError>(())
        }
        .await;
        let status = match camera_lifecycle::after_publication(result, stopped.is_cancelled()) {
            PublicationDecision::End => break,
            PublicationDecision::Retry(status) => status,
        };
        let _ = updates.send(status);
        if started.elapsed() >= Duration::from_secs(3) {
            backoff.reset();
        }
        tokio::select! {
            () = stopped.cancelled() => break,
            () = tokio::time::sleep(backoff.next_delay()) => {},
        }
    }
}

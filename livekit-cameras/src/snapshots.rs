use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Mutex, PoisonError},
    time::{Duration, Instant},
};

use crate::error::PublisherError;
use axum::{
    Json, Router,
    extract::{Path, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use hebbian_contracts::{CameraStatus, MediaState, MediaStatusReport, unix_nanoseconds};
use livekit::webrtc::video_frame::VideoRotation;
use livekit_camera_frames::{CameraFrame, CameraFrames, LatestFrame, TrackIdentity, camera_frame};
use livekit_camera_publisher::capture::CapturedI420Frame;
use serde::Deserialize;
use station_access::{Credentials, WatcherRole};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::UnixStream,
};
use tokio_util::sync::CancellationToken;

const MAX_SIMULATOR_HEADER_BYTES: u64 = 16 * 1024;
const MAX_SIMULATOR_FRAME_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Default)]
pub struct Snapshots {
    frames: CameraFrames,
    local_status: Arc<Mutex<BTreeMap<hebbian_contracts::CameraName, CameraStatus>>>,
    socket: Option<PathBuf>,
}
#[derive(Deserialize)]
struct SocketReply {
    names: Option<Vec<String>>,
    available: Option<Vec<String>>,
    bytes: Option<usize>,
    error: Option<String>,
}

impl Snapshots {
    pub fn start(
        names: Vec<hebbian_contracts::CameraName>,
        socket: Option<PathBuf>,
        room: Option<String>,
    ) -> Self {
        let snapshots = Self {
            frames: CameraFrames::default(),
            local_status: Arc::new(Mutex::new(
                names
                    .into_iter()
                    .map(|name| (name, CameraStatus::Resolving))
                    .collect(),
            )),
            socket,
        };
        if let Some(room) = room {
            let state = snapshots.clone();
            tokio::spawn(async move {
                state.subscribe(&room).await;
            });
        }
        snapshots
    }
    pub(super) fn set_camera_status(&self, name: &str, status: CameraStatus) {
        let mut statuses = self
            .local_status
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(previous) = statuses.get_mut(name) {
            *previous = status;
        }
    }

    pub(super) fn register_local_frames(&self, name: &str) {
        self.frames.register(TrackIdentity {
            participant: name.to_owned(),
            track_sid: format!("local:{name}"),
        });
    }

    pub(super) fn store_local_frame(&self, name: &str, frame: &CapturedI420Frame) {
        let Ok(planes) = frame.i420.planes() else {
            return;
        };
        let mut buffer = livekit::webrtc::video_frame::I420Buffer::with_strides(
            frame.i420.width,
            frame.i420.height,
            frame.i420.width,
            frame.i420.width.div_ceil(2),
            frame.i420.width.div_ceil(2),
        );
        let (y, u, v) = buffer.data_mut();
        y.copy_from_slice(planes.y);
        u.copy_from_slice(planes.u);
        v.copy_from_slice(planes.v);
        if let Ok(image) = camera_frame(
            &buffer,
            VideoRotation::VideoRotation0,
            frame.captured_at_unix_ns,
            Instant::now(),
        ) {
            self.frames.insert(&format!("local:{name}"), image);
        }
    }

    pub fn router(self) -> Router {
        Router::new()
            .route("/cameras", get(list))
            .route("/viewer/api/media-status", get(camera_status))
            .route("/cameras/{name}", get(snapshot))
            .with_state(self)
    }
    fn latest(&self, name: &str) -> Option<CameraFrame> {
        self.frames
            .tracks()
            .into_iter()
            .filter(|track| track.participant == name)
            .filter_map(|track| match self.frames.latest(&track.track_sid) {
                LatestFrame::Available(frame) => Some(frame),
                _ => None,
            })
            .max_by_key(|frame| frame.received_at)
    }
    async fn subscribe(&self, room_name: &str) {
        let mut backoff = hebbian_contracts::Backoff::new(Duration::from_secs(3));
        loop {
            match self.subscribe_once(room_name).await {
                Ok(()) => backoff.reset(),
                Err(error) => tracing::warn!(%error, "Camera subscription unavailable"),
            }
            tokio::time::sleep(backoff.next_delay()).await;
        }
    }
    async fn subscribe_once(&self, room_name: &str) -> Result<(), PublisherError> {
        let access =
            Credentials::from_environment()?.watcher_access(room_name, WatcherRole::Model)?;
        livekit_camera_frames::subscribe(
            &access.server_url,
            &access.token,
            self.frames.clone(),
            CancellationToken::new(),
        )
        .await?;
        Ok(())
    }
}
async fn ask_socket(path: &PathBuf, request: serde_json::Value) -> Option<(SocketReply, Vec<u8>)> {
    tokio::time::timeout(Duration::from_secs(7), async {
        let mut connection = UnixStream::connect(path).await.ok()?;
        connection
            .write_all(format!("{request}\n").as_bytes())
            .await
            .ok()?;
        let mut reader = BufReader::new(connection);
        let mut header = Vec::new();
        (&mut reader)
            .take(MAX_SIMULATOR_HEADER_BYTES)
            .read_until(b'\n', &mut header)
            .await
            .ok()?;
        if header.last() != Some(&b'\n') {
            return None;
        }
        let reply: SocketReply = serde_json::from_slice(&header).ok()?;
        let frame_bytes = reply.bytes.unwrap_or(0);
        if frame_bytes > MAX_SIMULATOR_FRAME_BYTES {
            return None;
        }
        let mut body = vec![0; frame_bytes];
        reader.read_exact(&mut body).await.ok()?;
        Some((reply, body))
    })
    .await
    .ok()
    .flatten()
}
fn frame_is_fresh(received_at: Instant, now: Instant) -> bool {
    now.saturating_duration_since(received_at) <= Duration::from_secs(3)
}

async fn camera_status(State(state): State<Snapshots>) -> Json<MediaStatusReport> {
    Json(MediaStatusReport {
        service: MediaState::Running,
        cameras: state
            .local_status
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone(),
    })
}

async fn list(State(state): State<Snapshots>) -> Json<Vec<hebbian_contracts::CameraListing>> {
    let mut names = BTreeMap::new();
    for name in state
        .local_status
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .keys()
    {
        names.insert(name.to_string(), false);
    }
    let now = Instant::now();
    for track in state.frames.tracks() {
        if cameras::validate_camera_name(&track.participant).is_ok() {
            let available = state
                .latest(&track.participant)
                .is_some_and(|frame| frame_is_fresh(frame.received_at, now));
            names.insert(track.participant, available);
        }
    }
    if let Some(path) = &state.socket
        && let Some((reply, _)) = ask_socket(path, serde_json::json!({"list":true})).await
    {
        let available = reply.available.unwrap_or_default();
        for name in reply.names.unwrap_or_default() {
            let fresh = available.contains(&name);
            names.insert(format!("sim_{name}"), fresh);
        }
    }
    Json(
        names
            .into_iter()
            .map(|(name, available)| hebbian_contracts::CameraListing { name, available })
            .collect(),
    )
}
async fn snapshot(State(state): State<Snapshots>, Path(name): Path<String>) -> Response {
    if let Some(view) = name.strip_prefix("sim_")
        && let Some(path) = &state.socket
    {
        if let Some((reply, jpeg)) = ask_socket(path, serde_json::json!({"camera":view})).await
            && reply.error.is_none()
            && reply.bytes.is_some()
        {
            return image_response(jpeg, u128::from(unix_nanoseconds()));
        }
        return StatusCode::NOT_FOUND.into_response();
    }
    let frame = state.latest(&name);
    let now = Instant::now();
    match frame {
        Some(frame) if frame_is_fresh(frame.received_at, now) => {
            image_response(frame.jpeg.to_vec(), frame.captured_at_unix_ns)
        }
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}
fn image_response(jpeg: Vec<u8>, timestamp: u128) -> Response {
    (
        [
            (header::CONTENT_TYPE, "image/jpeg".to_owned()),
            (
                header::HeaderName::from_static("x-captured-at-unix-ns"),
                timestamp.to_string(),
            ),
        ],
        jpeg,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn an_expected_camera_that_never_publishes_is_listed_unavailable()
    -> Result<(), Box<dyn std::error::Error>> {
        let snapshots = Snapshots::start(
            vec![hebbian_contracts::CameraName::try_from("wrist").unwrap()],
            None,
            None,
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let router = snapshots.router();
        let server = tokio::spawn(async move { axum::serve(listener, router).await });
        let listing: serde_json::Value = reqwest::get(format!("http://{address}/cameras"))
            .await?
            .json()
            .await?;
        assert_eq!(
            listing,
            serde_json::json!([{"name":"wrist","available":false}])
        );
        server.abort();
        Ok(())
    }

    #[tokio::test]
    async fn snapshot_routes_preserve_image_timestamp_and_reject_stale_frames()
    -> Result<(), Box<dyn std::error::Error>> {
        let snapshots = Snapshots::default();
        snapshots.frames.register(TrackIdentity {
            participant: "front".into(),
            track_sid: "track".into(),
        });
        let jpeg: Arc<[u8]> = Arc::from([1_u8, 2, 3]);
        let frame = CameraFrame {
            jpeg: jpeg.clone(),
            width: 1,
            height: 1,
            captured_at_unix_ns: 12345,
            received_at: Instant::now(),
        };
        snapshots.frames.insert("track", frame.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let router = snapshots.clone().router();
        let server = tokio::spawn(async move { axum::serve(listener, router).await });
        let client = reqwest::Client::new();
        let response = client
            .get(format!("http://{address}/cameras/front"))
            .send()
            .await?;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["content-type"], "image/jpeg");
        assert_eq!(response.headers()["x-captured-at-unix-ns"], "12345");
        assert_eq!(response.bytes().await?.as_ref(), jpeg.as_ref());
        snapshots.frames.insert(
            "track",
            CameraFrame {
                received_at: Instant::now()
                    .checked_sub(Duration::from_secs(4))
                    .ok_or("clock cannot represent stale frame")?,
                ..frame
            },
        );
        let listing: serde_json::Value = client
            .get(format!("http://{address}/cameras"))
            .send()
            .await?
            .json()
            .await?;
        assert_eq!(
            listing,
            serde_json::json!([{"name":"front","available":false}])
        );
        assert_eq!(
            client
                .get(format!("http://{address}/cameras/front"))
                .send()
                .await?
                .status(),
            StatusCode::NOT_FOUND
        );
        server.abort();
        Ok(())
    }
}

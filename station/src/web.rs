//! What the station serves beside its control API: recording, the required-camera monitor,
//! and `LiveKit` access for the viewer and phones, mounted on the executor's
//! HTTP server.
//!
//! - `GET /viewer/api/token`: watch-only access to every camera in the room.
//! - `POST /viewer/api/phone-invites`: a link and QR code for a phone camera.
//! - `/viewer/api/recording/...`: the recorder (`recording`).
//! - The required-camera monitor (`lease_routes`); camera reports and lease-checked stops use HTTP.
//! - `/viewer/api/recordings`: the `recordings`
//!   library over the recordings directory, read from disk on the server's own
//!   threads.
//! - `GET /viewer/api/control-mode`: whether the viewer shows manual controls.
//!
//! Nothing here lets a call that leaves the machine hold up control, `/events`,
//! `GET /state` or the viewer. Tokens are signed locally. The room announcement
//! of a recording runs on the recorder's own worker with a 10 s timeout, and
//! a room that cannot be reached marks the lease's required cameras unavailable
//! while the client continues. Without `LiveKit` credentials the token and invite
//! routes answer 503 and control goes on without cameras.
//!
//! The robot server never launches or records agent runs; an agent server
//! (or any client) owns the loop and its run records.

use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};

use crate::lease_routes::ServedArms;
use crate::recording::{RecordingConfiguration, RecordingManager};
use hebbian_contracts::{PhoneInvite, PhoneInviteRequest};
use hebbian_robot_use_server::control_link::LinkHandle;
use schemars::JsonSchema;
use serde::Serialize;
use station_access::{CameraConfiguration, Credentials, WatcherRole, new_room_name};

/// The hosted phone page; operators can supply their own HTTPS page instead.
pub const DEFAULT_PHONE_PAGE_URL: &str = "https://hebbian-robotics.github.io/livekit-cameras/";

fn local_media_server_url(address: SocketAddr) -> String {
    let ip = match address.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => Ipv4Addr::LOCALHOST.into(),
        IpAddr::V6(ip) if ip.is_unspecified() => Ipv6Addr::LOCALHOST.into(),
        ip => ip,
    };
    format!("http://{}", SocketAddr::new(ip, address.port()))
}

/// Where recordings live and how the station records and serves them.
#[derive(Debug, Clone)]
pub struct WebSettings {
    pub recordings_directory: PathBuf,
    pub enable_recording: bool,
    /// The media endpoint the camera monitor checks availability against, if served here.
    pub media_address: Option<SocketAddr>,
    /// The installed `hebbian-robot-use-media`, which records the cameras.
    pub media_executable: PathBuf,
    /// The `LiveKit` room every camera publishes to; a new one when none is named.
    pub room: Option<String>,
    pub phone_page_url: String,
    /// Whether the viewer shows its manual controls (a single SO-101 arm).
    pub manual_control: bool,
    /// The media supervisor reports here; `GET /viewer/api/media-status`.
    pub media_status: crate::media_status::MediaStatus,
}

/// What the viewer shows for driving the arm.
#[derive(Debug, Clone, Copy, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ViewerControlMode {
    /// A model loop drives; the viewer only watches.
    Llm,
    /// The viewer's pose controls drive a single SO-101.
    So101Manual,
}

/// `GET /viewer/api/control-mode`.
#[derive(Debug, Clone, Copy, Serialize, JsonSchema)]
pub struct ViewerControl {
    pub mode: ViewerControlMode,
}

enum Cameras {
    /// Why `LiveKit` is off: no credentials, or only some.
    Off(String),
    On(Credentials),
}

struct Access {
    cameras: Cameras,
    room: String,
    phone_page_url: String,
}

/// A refusal, answered as `{"detail": ...}` like the other routes'.
struct Refusal(StatusCode, String);

impl IntoResponse for Refusal {
    fn into_response(self) -> Response {
        (self.0, Json(serde_json::json!({ "detail": self.1 }))).into_response()
    }
}

fn credentials(access: &Access) -> Result<&Credentials, Refusal> {
    match &access.cameras {
        Cameras::On(credentials) => Ok(credentials),
        Cameras::Off(reason) => Err(Refusal(
            StatusCode::SERVICE_UNAVAILABLE,
            format!("Cameras are off: {reason}"),
        )),
    }
}

async fn create_token(State(access): State<Arc<Access>>) -> Result<Response, Refusal> {
    let granted = credentials(&access)?
        .watcher_access(&access.room, WatcherRole::Viewer)
        .map_err(|error| Refusal(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
    Ok((
        [(header::CACHE_CONTROL, HeaderValue::from_static("no-store"))],
        Json(granted),
    )
        .into_response())
}

async fn create_phone_invite(
    State(access): State<Arc<Access>>,
    Json(request): Json<PhoneInviteRequest>,
) -> Result<Json<PhoneInvite>, Refusal> {
    credentials(&access)?
        .phone_invite(&access.room, &request.label, &access.phone_page_url)
        .map(Json)
        .map_err(|error| Refusal(StatusCode::BAD_REQUEST, error.to_string()))
}

fn camera_access() -> Cameras {
    match CameraConfiguration::from_environment() {
        Ok(CameraConfiguration::Enabled(credentials)) => Cameras::On(credentials),
        Ok(CameraConfiguration::Off { reason }) => Cameras::Off(reason.to_string()),
        Err(error) => {
            eprintln!("LiveKit is not fully configured, so cameras are off: {error}");
            Cameras::Off(error.to_string())
        }
    }
}

/// What the mounted routes need once the arms are set up.
pub struct Mounted {
    pub recording: RecordingManager,
    pub arms: ServedArms,
    pub media_server_url: Option<String>,
}

impl Mounted {
    /// Serve these arms on the lease routes and watch their required cameras.
    pub fn attach_arms(
        &self,
        runtime: &tokio::runtime::Handle,
        arms: &BTreeMap<String, LinkHandle>,
    ) {
        let _ = self.arms.set(arms.clone());
        if let Some(link) = arms.values().next() {
            runtime.spawn(crate::lease_routes::monitor_required_cameras(
                link.clone(),
                self.media_server_url.clone(),
            ));
        }
    }
}

/// The routes and what they serve, created on the server's runtime.
pub fn mount(
    runtime: &tokio::runtime::Handle,
    address: SocketAddr,
    settings: WebSettings,
) -> (Router, Mounted) {
    let room = settings.room.unwrap_or_else(new_room_name);
    let recording_library =
        recordings::RecordingLibrary::new(settings.recordings_directory.clone());
    let credentials = if settings.enable_recording {
        match Credentials::from_environment() {
            Ok(credentials) => match std::process::Command::new("ffmpeg")
                .arg("-version")
                .output()
            {
                Ok(output) if output.status.success() => Some(credentials),
                _ => {
                    eprintln!(
                        "Recording disabled: install ffmpeg and put it on PATH (docs/recording.md)"
                    );
                    None
                }
            },
            Err(error) => {
                eprintln!("Recording disabled: LiveKit credentials are unavailable: {error}");
                None
            }
        }
    } else {
        None
    };
    let recording = credentials.map(|credentials| RecordingConfiguration {
        credentials,
        room: room.clone(),
        directory: settings.recordings_directory,
        executable: settings.media_executable,
    });
    let recording_enabled = recording.is_some();
    let station_url = format!("http://127.0.0.1:{}", address.port());
    let media_server_url = settings.media_address.map(local_media_server_url);
    // The manager spawns its recording announcer, so it needs the server's runtime.
    let _runtime = runtime.enter();
    let manager = RecordingManager::new(recording, station_url);
    let arms = ServedArms::default();
    let recording_manager = manager.clone();
    let recording_library =
        recording_library.with_active_recording(move || recording_manager.active_recording());
    let access = Arc::new(Access {
        cameras: camera_access(),
        room,
        phone_page_url: settings.phone_page_url,
    });
    let media_status = settings.media_status;
    let control_mode = ViewerControl {
        mode: if settings.manual_control {
            ViewerControlMode::So101Manual
        } else {
            ViewerControlMode::Llm
        },
    };
    let router = Router::new()
        .route(
            "/viewer/api/recording-enabled",
            get(move || async move { Json(recording_enabled) }),
        )
        .route("/viewer/api/token", get(create_token))
        .route("/viewer/api/phone-invites", post(create_phone_invite))
        .with_state(access)
        .route(
            "/viewer/api/control-mode",
            get(move || async move { Json(control_mode) }),
        )
        .route(
            "/viewer/api/media-status",
            get(move || async move { Json(media_status.all()) }),
        )
        .nest(
            "/viewer/api",
            crate::recording::router(manager.clone(), arms.clone()),
        )
        .nest("/viewer/api", recordings::router(recording_library));
    (
        router,
        Mounted {
            recording: manager,
            arms,
            media_server_url,
        },
    )
}

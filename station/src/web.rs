// CONSTRAINT: External services must not block control or local reads. Tokens are signed
// locally; recording announcements use a bounded worker, and unavailable cameras
// raise lease interlocks rather than blocking station request handling.

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
use hebbian_contracts::{PhoneInvite, PhoneInviteRequest, ViewerControl, ViewerControlMode};
use hebbian_robot_use_core::control_link::LinkHandle;
use station_access::{CameraConfiguration, Credentials, WatcherRole, new_room_name};

pub const DEFAULT_PHONE_PAGE_URL: &str = "https://hebbian-robotics.github.io/livekit-cameras/";

fn local_media_server_url(address: SocketAddr) -> String {
    let ip = match address.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => Ipv4Addr::LOCALHOST.into(),
        IpAddr::V6(ip) if ip.is_unspecified() => Ipv6Addr::LOCALHOST.into(),
        ip => ip,
    };
    format!("http://{}", SocketAddr::new(ip, address.port()))
}

#[derive(Debug, Clone)]
pub struct WebSettings {
    pub recordings_directory: PathBuf,
    pub enable_recording: bool,
    pub media_address: Option<SocketAddr>,
    pub media_executable: PathBuf,
    pub room: Option<String>,
    pub phone_page_url: String,
    pub manual_control: bool,
    pub media_status: crate::media_status::MediaStatus,
}

enum Cameras {
    Off(String),
    On(Credentials),
}

struct Access {
    cameras: Cameras,
    room: String,
    phone_page_url: String,
}

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

pub struct Mounted {
    pub recording: RecordingManager,
    pub arms: ServedArms,
    pub media_server_url: Option<String>,
}

impl Mounted {
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
    // WHY: The manager spawns its recording announcer, so it needs the server's runtime.
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
            get(move || async move { Json(media_status.report()) }),
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

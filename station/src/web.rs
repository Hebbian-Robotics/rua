// CONSTRAINT: External services must not block control or local reads. Tokens are signed
// locally; recording announcements use a bounded worker, and unavailable cameras
// raise lease interlocks rather than blocking station request handling.

use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, ensure};
use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};

use crate::lease_routes::{Refusal, ServedArms, refusal};
use crate::recording::{RecordingConfiguration, RecordingManager};
use hebbian_contracts::{PhoneInvite, PhoneInviteRequest};
use hebbian_rua_core::control_link::LinkHandle;
use station_access::{Credentials, WatcherRole, new_room_name};

pub const DEFAULT_PHONE_PAGE_URL: &str = "https://hebbian-robotics.github.io/livekit-cameras/";

fn connectable_local_address(address: SocketAddr) -> SocketAddr {
    let ip = match address.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => Ipv4Addr::LOCALHOST.into(),
        IpAddr::V6(ip) if ip.is_unspecified() => Ipv6Addr::LOCALHOST.into(),
        ip => ip,
    };
    SocketAddr::new(ip, address.port())
}

#[must_use]
pub fn local_http_url(address: SocketAddr) -> String {
    format!("http://{}", connectable_local_address(address))
}

#[derive(Debug, Clone, Copy)]
pub enum CameraForwarding {
    Enabled,
    DirectOnly,
}

#[derive(Debug, Clone)]
pub struct WebSettings {
    pub local_cameras_only: bool,
    pub recordings_directory: PathBuf,
    pub enable_recording: bool,
    pub media_address: Option<SocketAddr>,
    pub camera_forwarding: CameraForwarding,
    pub media_executable: PathBuf,
    pub room: Option<String>,
    pub phone_page_url: String,
    pub media_status: crate::media_status::MediaStatus,
}

pub enum MediaCapabilities {
    LocalOnly,
    LiveKit {
        credentials: Credentials,
        recording: bool,
    },
}

impl MediaCapabilities {
    // INVARIANT: Resolve startup requirements once for the station and media companion.
    pub fn resolve(settings: &WebSettings) -> anyhow::Result<Self> {
        if settings.local_cameras_only {
            return Ok(Self::LocalOnly);
        }
        let credentials = Credentials::from_environment().context(
            "LiveKit is required for station startup. Run lk cloud auth or configure the LiveKit environment variables. For local cameras only, pass --local-only-no-remote-cameras",
        )?;
        if settings.enable_recording {
            let requirement =
                "Recording requires FFmpeg on PATH. Install FFmpeg or pass --no-recording";
            let version = std::process::Command::new("ffmpeg")
                .arg("-version")
                .output()
                .context(requirement)?;
            ensure!(version.status.success(), "{requirement}");
        }
        Ok(Self::LiveKit {
            credentials,
            recording: settings.enable_recording,
        })
    }

    #[must_use]
    pub fn remote_cameras(&self) -> bool {
        matches!(self, Self::LiveKit { .. })
    }
}

struct Access {
    cameras: MediaCapabilities,
    room: String,
    phone_page_url: String,
}

fn credentials(access: &Access) -> Result<&Credentials, Refusal> {
    match &access.cameras {
        MediaCapabilities::LiveKit { credentials, .. } => Ok(credentials),
        MediaCapabilities::LocalOnly => Err(refusal(
            StatusCode::CONFLICT,
            "Local cameras only. Remote cameras require LiveKit.",
        )),
    }
}

async fn create_token(State(access): State<Arc<Access>>) -> Result<Response, Refusal> {
    let granted = credentials(&access)?
        .watcher_access(&access.room, WatcherRole::Viewer)
        .map_err(|error| refusal(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
    Ok((
        [(header::CACHE_CONTROL, HeaderValue::from_static("no-store"))],
        Json(granted),
    )
        .into_response())
}

async fn create_phone_invite(
    State(access): State<Arc<Access>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Json(request): Json<PhoneInviteRequest>,
) -> Result<Response, Refusal> {
    if !peer.ip().to_canonical().is_loopback() {
        return Err(refusal(
            StatusCode::FORBIDDEN,
            "Create phone camera invites from the station itself",
        ));
    }
    let invite: PhoneInvite = credentials(&access)?
        .phone_invite(&access.room, &request.label, &access.phone_page_url)
        .map_err(|error| {
            let status = match error {
                station_access::AccessError::PhoneCannotReachServer(_) => StatusCode::CONFLICT,
                _ => StatusCode::BAD_REQUEST,
            };
            refusal(status, error.to_string())
        })?;
    Ok((
        [(header::CACHE_CONTROL, HeaderValue::from_static("no-store"))],
        Json(invite),
    )
        .into_response())
}

pub struct Mounted {
    pub recording: RecordingManager,
    pub arms: ServedArms,
    camera_proxy: Option<Arc<crate::camera_proxy::CameraProxy>>,
    media_connect_address: Option<SocketAddr>,
}

impl Mounted {
    pub fn attach_sequence_cameras(&self, server: &hebbian_rua_core::station_api::HttpServer) {
        if let Some(proxy) = &self.camera_proxy {
            let proxy = Arc::clone(proxy);
            server.attach_sequence_cameras(Arc::new(move |name, maximum_bytes| {
                let proxy = Arc::clone(&proxy);
                Box::pin(async move {
                    proxy
                        .capture_frame(&name, std::time::Duration::from_millis(500), maximum_bytes)
                        .await
                })
            }));
        }
    }

    pub fn attach_arms(
        &self,
        runtime: &tokio::runtime::Handle,
        arms: &BTreeMap<String, LinkHandle>,
    ) {
        let _ = self.arms.set(arms.clone());
        if let Some(link) = arms.values().next() {
            runtime.spawn(crate::lease_routes::monitor_required_cameras(
                link.clone(),
                self.media_connect_address
                    .map(crate::camera_proxy::camera_list_url),
            ));
        }
    }
}

async fn media_status_response(
    media_status: &crate::media_status::MediaStatus,
    proxy: Option<&crate::camera_proxy::CameraProxy>,
) -> Result<Json<hebbian_contracts::MediaStatusReport>, Refusal> {
    media_status
        .report_with_cameras(proxy)
        .await
        .map(Json)
        .map_err(|error| {
            tracing::debug!(%error, "Camera status unavailable");
            refusal(
                StatusCode::SERVICE_UNAVAILABLE,
                "Camera status could not be refreshed".to_owned(),
            )
        })
}

pub fn mount(
    runtime: &tokio::runtime::Handle,
    address: SocketAddr,
    settings: WebSettings,
    capabilities: MediaCapabilities,
    controller_available: impl Fn() -> bool + Send + Sync + 'static,
) -> (Router, Mounted) {
    let room = settings.room.unwrap_or_else(new_room_name);
    let recording_library =
        recordings::RecordingLibrary::new(settings.recordings_directory.clone());
    let recording = match &capabilities {
        MediaCapabilities::LiveKit {
            credentials,
            recording: true,
        } => Some(RecordingConfiguration {
            credentials: credentials.clone(),
            room: room.clone(),
            directory: settings.recordings_directory,
            executable: settings.media_executable,
        }),
        _ => None,
    };
    let recording_enabled = recording.is_some();
    let station_url = local_http_url(address);
    let media_connect_address = settings.media_address.map(connectable_local_address);
    // WHY: The manager spawns its recording announcer, so it needs the server's runtime.
    let _runtime = runtime.enter();
    let manager = RecordingManager::new(recording, station_url, controller_available);
    let arms = ServedArms::default();
    let recording_manager = manager.clone();
    let recording_library =
        recording_library.with_active_recording(move || recording_manager.active_recording());
    let access = Arc::new(Access {
        cameras: capabilities,
        room,
        phone_page_url: settings.phone_page_url,
    });
    let media_status = settings.media_status;
    let proxy = crate::camera_proxy::camera_proxy(media_connect_address);
    let status_proxy = proxy.clone();
    let router = Router::new()
        .route(
            "/recording/enabled",
            get(move || async move { Json(recording_enabled) }),
        )
        .route("/media/access-token", get(create_token))
        .route("/media/phone-invites", post(create_phone_invite))
        .with_state(access)
        .route(
            "/media/status",
            get(move || async move {
                media_status_response(&media_status, status_proxy.as_deref()).await
            }),
        )
        .merge(crate::recording::router(manager.clone(), arms.clone()))
        .merge(recordings::router(recording_library));
    let router = router.merge(crate::camera_proxy::observation_router(
        arms.clone(),
        proxy.clone(),
    ));
    let router = match settings.camera_forwarding {
        CameraForwarding::Enabled => router.merge(crate::camera_proxy::router(proxy.clone())),
        CameraForwarding::DirectOnly => router,
    };
    (
        router,
        Mounted {
            recording: manager,
            arms,
            camera_proxy: proxy,
            media_connect_address,
        },
    )
}

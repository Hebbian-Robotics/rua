use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use crate::lease_routes::ServedArms;
use axum::Json;
use axum::Router;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::routing::post;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use hebbian_contracts::{ArmMode, ObservationAnswer, ObservationFrame, ObservationRequest};
use std::collections::{BTreeMap, BTreeSet};
use tokio::sync::Semaphore;

use crate::lease_routes::refusal;

const MAX_IN_FLIGHT: usize = 4;
const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const UPSTREAM_TIMEOUT: Duration = Duration::from_millis(750);
const CAPTURED_AT: &str = "x-captured-at-unix-ns";

pub(crate) struct CameraProxy {
    camera_list_url: reqwest::Url,
    client: reqwest::Client,
    in_flight: Semaphore,
}

enum CameraRequest<'a> {
    List,
    Snapshot(&'a str),
}

enum ForwardingOutcome {
    Received(Response),
    SnapshotNotFound,
    CompanionUnavailable,
    Busy,
    TimedOut,
    ResponseTooLarge,
}

impl IntoResponse for ForwardingOutcome {
    fn into_response(self) -> Response {
        match self {
            Self::Received(response) => response,
            Self::SnapshotNotFound => StatusCode::NOT_FOUND.into_response(),
            Self::CompanionUnavailable => refusal(
                StatusCode::SERVICE_UNAVAILABLE,
                "Media companion is unavailable",
            )
            .into_response(),
            Self::Busy => refusal(StatusCode::SERVICE_UNAVAILABLE, "Camera forwarding is busy")
                .into_response(),
            Self::TimedOut => {
                refusal(StatusCode::GATEWAY_TIMEOUT, "Media companion timed out").into_response()
            }
            Self::ResponseTooLarge => {
                refusal(StatusCode::BAD_GATEWAY, "Media response is too large").into_response()
            }
        }
    }
}

impl CameraProxy {
    fn new(media_address: SocketAddr) -> anyhow::Result<Self> {
        Ok(Self {
            camera_list_url: reqwest::Url::parse(&camera_list_url(media_address))?,
            client: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
            in_flight: Semaphore::new(MAX_IN_FLIGHT),
        })
    }

    pub(crate) async fn camera_status(
        &self,
    ) -> reqwest::Result<std::collections::BTreeMap<String, hebbian_contracts::CameraStatus>> {
        let mut url = self.camera_list_url.clone();
        url.set_path("/viewer/api/media-status");
        self.client
            .get(url)
            .timeout(Duration::from_secs(1))
            .send()
            .await?
            .error_for_status()?
            .json::<hebbian_contracts::MediaStatusReport>()
            .await
            .map(|status| status.cameras)
    }

    async fn fetch(&self, request: CameraRequest<'_>) -> anyhow::Result<ForwardingOutcome> {
        let mut url = self.camera_list_url.clone();
        if let CameraRequest::Snapshot(name) = request {
            url.path_segments_mut()
                .map_err(|()| anyhow::anyhow!("Media companion URL cannot hold a path"))?
                .push(name);
        }

        let mut upstream = self.client.get(url).send().await?;
        let mut headers = axum::http::HeaderMap::new();
        for name in [
            header::CONTENT_TYPE,
            header::HeaderName::from_static(CAPTURED_AT),
        ] {
            if let Some(value) = upstream.headers().get(&name) {
                headers.insert(name, value.clone());
            }
        }
        headers.insert(
            header::CACHE_CONTROL,
            header::HeaderValue::from_static("no-store"),
        );
        let mut body = Vec::new();
        while let Some(chunk) = upstream.chunk().await? {
            if chunk.len() > MAX_RESPONSE_BYTES - body.len() {
                return Ok(ForwardingOutcome::ResponseTooLarge);
            }
            body.extend_from_slice(&chunk);
        }
        Ok(ForwardingOutcome::Received(
            (upstream.status(), headers, body).into_response(),
        ))
    }

    async fn forward(&self, request: CameraRequest<'_>) -> anyhow::Result<ForwardingOutcome> {
        // CONSTRAINT: Shed excess camera requests immediately so they do not queue on the station.
        let Ok(_permit) = self.in_flight.try_acquire() else {
            return Ok(ForwardingOutcome::Busy);
        };
        match tokio::time::timeout(UPSTREAM_TIMEOUT, self.fetch(request)).await {
            Ok(outcome) => outcome,
            Err(_) => Ok(ForwardingOutcome::TimedOut),
        }
    }
}

pub(crate) fn camera_list_url(media_address: SocketAddr) -> String {
    format!("http://{media_address}/cameras")
}

pub(crate) fn camera_proxy(media_address: Option<SocketAddr>) -> Option<Arc<CameraProxy>> {
    match media_address.map(CameraProxy::new).transpose() {
        Ok(proxy) => proxy.map(Arc::new),
        Err(error) => {
            tracing::warn!(%error, "Camera forwarding unavailable");
            None
        }
    }
}

pub(crate) fn router(proxy: Option<Arc<CameraProxy>>) -> Router {
    Router::new()
        .route("/cameras", get(list_cameras))
        .route("/cameras/{name}", get(snapshot))
        .with_state(proxy)
}

async fn list_cameras(State(proxy): State<Option<Arc<CameraProxy>>>) -> Response {
    forward(proxy.as_deref(), CameraRequest::List).await
}

async fn snapshot(
    State(proxy): State<Option<Arc<CameraProxy>>>,
    Path(name): Path<String>,
) -> Response {
    // WHY: URL path construction normalizes dot segments. Simulator names are otherwise opaque.
    if matches!(name.as_str(), "." | "..") {
        return ForwardingOutcome::SnapshotNotFound.into_response();
    }
    forward(proxy.as_deref(), CameraRequest::Snapshot(&name)).await
}

async fn forward(proxy: Option<&CameraProxy>, request: CameraRequest<'_>) -> Response {
    let Some(proxy) = proxy else {
        return ForwardingOutcome::CompanionUnavailable.into_response();
    };
    match proxy.forward(request).await {
        Ok(outcome) => outcome.into_response(),
        Err(_) => {
            refusal(StatusCode::BAD_GATEWAY, "Media companion request failed").into_response()
        }
    }
}

#[derive(Clone)]
struct ObservationSource {
    arms: ServedArms,
    cameras: Option<Arc<CameraProxy>>,
}

pub(crate) fn observation_router(arms: ServedArms, cameras: Option<Arc<CameraProxy>>) -> Router {
    Router::new()
        .route("/observations", post(observe))
        .with_state(ObservationSource { arms, cameras })
}

const MAX_OBSERVATION_CAMERAS: usize = 8;
const OPTIONAL_TIMEOUT: Duration = Duration::from_millis(500);
const REQUIRED_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone, Copy)]
enum CameraImportance {
    Required,
    Optional,
}

struct ObservationSelection {
    arms: BTreeSet<String>,
    cameras: Vec<(String, CameraImportance)>,
}

impl TryFrom<ObservationRequest> for ObservationSelection {
    type Error = anyhow::Error;

    fn try_from(request: ObservationRequest) -> Result<Self, Self::Error> {
        let arms = request.arms.into_iter().collect();
        let mut names = BTreeSet::new();
        let mut cameras = Vec::new();
        for (selection, importance) in [
            (request.required_cameras, CameraImportance::Required),
            (request.optional_cameras, CameraImportance::Optional),
        ] {
            for name in selection {
                anyhow::ensure!(
                    !matches!(name.as_str(), "" | "." | "..") && names.insert(name.clone()),
                    "Expected unique nonempty camera names"
                );
                anyhow::ensure!(
                    names.len() <= MAX_OBSERVATION_CAMERAS,
                    "Too many cameras for one observation"
                );
                cameras.push((name, importance));
            }
        }
        Ok(Self { arms, cameras })
    }
}

async fn capture_frame(
    proxy: &CameraProxy,
    name: &str,
    importance: CameraImportance,
) -> Option<ObservationFrame> {
    let timeout = match importance {
        CameraImportance::Required => REQUIRED_TIMEOUT,
        CameraImportance::Optional => OPTIONAL_TIMEOUT,
    };
    tokio::time::timeout(timeout, async {
        let _permit = proxy.in_flight.acquire().await.ok()?;
        let started = hebbian_contracts::unix_nanoseconds();
        let ForwardingOutcome::Received(response) =
            proxy.fetch(CameraRequest::Snapshot(name)).await.ok()?
        else {
            return None;
        };
        if !response.status().is_success()
            || response
                .headers()
                .get(header::CONTENT_TYPE)?
                .to_str()
                .ok()?
                != "image/jpeg"
        {
            return None;
        }
        let captured_at_unix_ns = response
            .headers()
            .get(CAPTURED_AT)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok())
            .map(|nanos| nanos.to_string());
        let jpeg = axum::body::to_bytes(response.into_body(), MAX_RESPONSE_BYTES)
            .await
            .ok()?;
        let finished = hebbian_contracts::unix_nanoseconds();
        Some(ObservationFrame {
            jpeg_base64: STANDARD.encode(jpeg),
            captured_at_unix_ns,
            retrieved_between_unix_seconds: [started, finished]
                .map(|nanos| Duration::from_nanos(nanos).as_secs_f64()),
        })
    })
    .await
    .ok()
    .flatten()
}

async fn observe(
    State(source): State<ObservationSource>,
    Json(request): Json<ObservationRequest>,
) -> Result<Json<ObservationAnswer>, (StatusCode, String)> {
    let selection = ObservationSelection::try_from(request)
        .map_err(|error| (StatusCode::BAD_REQUEST, error.to_string()))?;
    let links = source.arms.get().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "Station arms are not ready".to_owned(),
    ))?;
    if let Some(arm) = selection.arms.iter().find(|arm| !links.contains_key(*arm)) {
        return Err((StatusCode::BAD_REQUEST, format!("Unknown arm {arm}")));
    }
    let state_unavailable = || {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "Station state is not ready".to_owned(),
        )
    };
    let mut state = hebbian_robot_use_core::station_api::station_state(links)
        .await
        .ok_or_else(state_unavailable)?;
    state
        .arms
        .retain(|arm| selection.arms.is_empty() || selection.arms.contains(&arm.arm));
    if let Some(arm) = state
        .arms
        .iter()
        .find(|arm| matches!(arm.mode, ArmMode::Disconnected | ArmMode::Reconnected))
    {
        return Ok(Json(ObservationAnswer::ObservationArmDisconnected {
            arm: arm.arm.clone(),
        }));
    }
    let mut captures = tokio::task::JoinSet::new();
    for (name, importance) in selection.cameras {
        let proxy = source.cameras.clone();
        captures.spawn(async move {
            let frame = if let Some(proxy) = proxy {
                capture_frame(&proxy, &name, importance).await
            } else {
                None
            };
            (name, importance, frame)
        });
    }
    let mut cameras = BTreeMap::new();
    let mut missing_optional_cameras = Vec::new();
    while let Some(capture) = captures.join_next().await {
        let (name, importance, frame) = capture.map_err(|_| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "Camera observation failed".to_owned(),
            )
        })?;
        match (importance, frame) {
            (_, Some(frame)) => {
                cameras.insert(name, frame);
            }
            (CameraImportance::Required, None) => {
                return Ok(Json(
                    ObservationAnswer::ObservationRequiredCameraUnavailable { camera: name },
                ));
            }
            (CameraImportance::Optional, None) => missing_optional_cameras.push(name),
        }
    }
    missing_optional_cameras.sort();
    for arm in &state.arms {
        let link = links.get(&arm.arm).ok_or_else(state_unavailable)?;
        if link.interlocks().1 != arm.revision {
            return Ok(Json(ObservationAnswer::ObservationRevisionChanged {
                arm: arm.arm.clone(),
            }));
        }
    }
    Ok(Json(ObservationAnswer::ObservationCaptured {
        state: Box::new(state),
        cameras,
        missing_optional_cameras,
    }))
}

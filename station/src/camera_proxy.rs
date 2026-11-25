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
    LocalList,
    Snapshot(&'a hebbian_contracts::SnapshotName),
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
    ) -> reqwest::Result<
        std::collections::BTreeMap<hebbian_contracts::CameraName, hebbian_contracts::CameraStatus>,
    > {
        let mut url = self.camera_list_url.clone();
        url.set_path("/media/status");
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

    async fn fetch(
        &self,
        request: CameraRequest<'_>,
        maximum_bytes: usize,
    ) -> anyhow::Result<ForwardingOutcome> {
        let mut url = self.camera_list_url.clone();
        match request {
            CameraRequest::Snapshot(name) => {
                url.path_segments_mut()
                    .map_err(|()| anyhow::anyhow!("Media companion URL cannot hold a path"))?
                    .push(name.as_str());
            }
            CameraRequest::LocalList => url.set_path("/media/local-cameras"),
            CameraRequest::List => {}
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
            if chunk.len() > maximum_bytes - body.len() {
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
        match tokio::time::timeout(UPSTREAM_TIMEOUT, self.fetch(request, MAX_RESPONSE_BYTES)).await
        {
            Ok(outcome) => outcome,
            Err(_) => Ok(ForwardingOutcome::TimedOut),
        }
    }

    pub(crate) async fn capture_frame(
        &self,
        name: &hebbian_contracts::SnapshotName,
        timeout: Duration,
        maximum_bytes: usize,
    ) -> Option<ObservationFrame> {
        tokio::time::timeout(timeout, async {
            let _permit = self.in_flight.acquire().await.ok()?;
            let started = hebbian_contracts::UnixNanos::now();
            let ForwardingOutcome::Received(response) = self
                .fetch(CameraRequest::Snapshot(name), maximum_bytes)
                .await
                .ok()?
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
            let jpeg = axum::body::to_bytes(response.into_body(), maximum_bytes)
                .await
                .ok()?;
            let finished = hebbian_contracts::UnixNanos::now();
            Some(ObservationFrame {
                jpeg_base64: STANDARD.encode(jpeg),
                captured_at_unix_ns,
                retrieved_between_unix_seconds: [started, finished]
                    .map(hebbian_contracts::UnixNanos::as_unix_seconds),
            })
        })
        .await
        .ok()
        .flatten()
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
        .route("/media/local-cameras", get(list_local_cameras))
        .with_state(proxy)
}

async fn list_cameras(State(proxy): State<Option<Arc<CameraProxy>>>) -> Response {
    forward(proxy.as_deref(), CameraRequest::List).await
}

async fn list_local_cameras(State(proxy): State<Option<Arc<CameraProxy>>>) -> Response {
    forward(proxy.as_deref(), CameraRequest::LocalList).await
}

async fn snapshot(
    State(proxy): State<Option<Arc<CameraProxy>>>,
    Path(name): Path<String>,
) -> Response {
    let Ok(name) = hebbian_contracts::SnapshotName::try_from(name) else {
        return ForwardingOutcome::SnapshotNotFound.into_response();
    };
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
    cameras: Vec<(hebbian_contracts::SnapshotName, CameraImportance)>,
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
                    names.insert(name.clone()),
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

async fn observe(
    State(source): State<ObservationSource>,
    Json(request): Json<ObservationRequest>,
) -> Result<Json<ObservationAnswer>, (StatusCode, String)> {
    let wait = request.wait.clone();
    let selection = ObservationSelection::try_from(request)
        .map_err(|error| (StatusCode::BAD_REQUEST, error.to_string()))?;
    let links = source.arms.get().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "Station arms are not ready".to_owned(),
    ))?;
    if let Some(arm) = selection.arms.iter().find(|arm| !links.contains_key(*arm)) {
        return Err((StatusCode::BAD_REQUEST, format!("Unknown arm {arm}")));
    }
    if let Some(wait) = wait {
        let link = links
            .get(&wait.arm)
            .filter(|_| selection.arms.is_empty() || selection.arms.contains(&wait.arm))
            .ok_or((
                StatusCode::BAD_REQUEST,
                format!("Unknown or unselected arm {}", wait.arm),
            ))?;
        return wait_for_observation(&source, links, &selection, &wait, link)
            .await
            .map(Json);
    }
    capture_observation(&source, links, &selection, None)
        .await
        .map(Json)
}

type ObservationResult = Result<ObservationAnswer, (StatusCode, String)>;
type ObservationLinks = BTreeMap<String, hebbian_rua_core::control_link::LinkHandle>;

fn lease_lost() -> (StatusCode, String) {
    (
        StatusCode::FORBIDDEN,
        "Observation lease is no longer held".to_owned(),
    )
}

async fn wait_for_observation(
    source: &ObservationSource,
    links: &ObservationLinks,
    selection: &ObservationSelection,
    wait: &hebbian_contracts::ObservationWait,
    link: &hebbian_rua_core::control_link::LinkHandle,
) -> ObservationResult {
    if !(1..=5000).contains(&wait.timeout_milliseconds) {
        return Err((
            StatusCode::BAD_REQUEST,
            "Observation wait must be 1–5000 ms".to_owned(),
        ));
    }
    let token = &wait.lease_token;
    let mut changes = link.authority_changes();
    link.with_lease(token, || ()).ok_or_else(lease_lost)?;
    let lost = async {
        while changes.changed().await.is_ok() {
            if link.with_lease(token, || ()).is_none() {
                break;
            }
        }
    };
    let capture = async {
        loop {
            link.with_lease(token, || ()).ok_or_else(lease_lost)?;
            let answer = capture_observation(source, links, selection, Some(&wait.arm)).await;
            match answer {
                Ok(ObservationAnswer::ObservationCaptured {
                    state,
                    cameras,
                    missing_optional_cameras,
                }) => {
                    for name in cameras.keys() {
                        if selection.cameras.iter().any(|(camera, importance)| {
                            camera.as_str() == name
                                && matches!(importance, CameraImportance::Required)
                        }) {
                            report_observation_camera(link, token, name, true)?;
                        }
                    }
                    let Some(latest) = hebbian_rua_core::station_api::station_state(links).await
                    else {
                        continue;
                    };
                    let Some(selected) = latest.arms.iter().find(|arm| arm.arm == wait.arm) else {
                        continue;
                    };
                    if let Some(outcome) = arm_recovery_outcome(selected) {
                        if matches!(outcome, ObservationAnswer::ObservationRecoveryEnded { .. }) {
                            return Ok(outcome);
                        }
                        continue;
                    }
                    // INVARIANT: A recovered camera can clear its interlock without changing
                    // the revision. Refresh the interlocks under the captured revision.
                    let mut state = state;
                    let mut stable = true;
                    for arm in &mut state.arms {
                        let (interlocks, revision) = links[&arm.arm].interlocks();
                        stable &= arm.revision == revision;
                        arm.interlocks = interlocks;
                    }
                    if stable
                        && state
                            .arms
                            .iter()
                            .any(|arm| arm.arm == wait.arm && arm.interlocks.is_empty())
                    {
                        link.with_lease(token, || ()).ok_or_else(lease_lost)?;
                        return Ok(ObservationAnswer::ObservationCaptured {
                            state,
                            cameras,
                            missing_optional_cameras,
                        });
                    }
                }
                Ok(ObservationAnswer::ObservationRequiredCameraUnavailable { camera }) => {
                    report_observation_camera(link, token, &camera, false)?;
                }
                Ok(
                    ObservationAnswer::ObservationArmDisconnected { .. }
                    | ObservationAnswer::ObservationRevisionChanged { .. },
                )
                | Err((StatusCode::SERVICE_UNAVAILABLE, _)) => {}
                other => return other,
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    };
    tokio::select! {
        biased;
        () = lost => Err(lease_lost()),
        answer = tokio::time::timeout(Duration::from_millis(u64::from(wait.timeout_milliseconds)), capture) => {
            link.with_lease(token, || ()).ok_or_else(lease_lost)?;
            answer.unwrap_or(Ok(ObservationAnswer::ObservationWaitTimedOut))
        }
    }
}

fn report_observation_camera(
    link: &hebbian_rua_core::control_link::LinkHandle,
    token: &str,
    name: &str,
    available: bool,
) -> Result<(), (StatusCode, String)> {
    use hebbian_rua_core::lease::CameraReportRefusal;
    match link.report_camera(token, name, available) {
        Ok(()) | Err(CameraReportRefusal::CameraNotRequired) => Ok(()),
        Err(CameraReportRefusal::LeaseLost) => Err(lease_lost()),
    }
}

fn arm_recovery_outcome(arm: &hebbian_contracts::ArmState) -> Option<ObservationAnswer> {
    use hebbian_contracts::Interlock;
    if matches!(arm.mode, ArmMode::Faulted | ArmMode::Disabled) {
        return Some(ObservationAnswer::ObservationRecoveryEnded {
            arm: arm.arm.clone(),
            mode: arm.mode,
        });
    }
    let recovering = matches!(arm.mode, ArmMode::Disconnected | ArmMode::Reconnected)
        || arm.interlocks.iter().any(|interlock| {
            matches!(
                interlock,
                Interlock::ArmDisconnected | Interlock::RecoveryApprovalRequired { .. }
            )
        });
    recovering.then(|| ObservationAnswer::ObservationArmDisconnected {
        arm: arm.arm.clone(),
    })
}

async fn capture_observation(
    source: &ObservationSource,
    links: &ObservationLinks,
    selection: &ObservationSelection,
    wait_arm: Option<&str>,
) -> ObservationResult {
    let state_unavailable = || {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "Station state is not ready".to_owned(),
        )
    };
    let mut state = hebbian_rua_core::station_api::station_state(links)
        .await
        .ok_or_else(state_unavailable)?;
    state
        .arms
        .retain(|arm| selection.arms.is_empty() || selection.arms.contains(&arm.arm));
    for arm in &state.arms {
        if wait_arm.is_some_and(|selected| selected != arm.arm) {
            continue;
        }
        if wait_arm.is_some() {
            if let Some(outcome) = arm_recovery_outcome(arm) {
                return Ok(outcome);
            }
        } else if matches!(arm.mode, ArmMode::Disconnected | ArmMode::Reconnected) {
            return Ok(ObservationAnswer::ObservationArmDisconnected {
                arm: arm.arm.clone(),
            });
        }
    }
    let mut captures = tokio::task::JoinSet::new();
    for (name, importance) in selection.cameras.iter().cloned() {
        let proxy = source.cameras.clone();
        captures.spawn(async move {
            let frame = if let Some(proxy) = proxy {
                let timeout = match importance {
                    CameraImportance::Required => REQUIRED_TIMEOUT,
                    CameraImportance::Optional => OPTIONAL_TIMEOUT,
                };
                proxy
                    .capture_frame(&name, timeout, MAX_RESPONSE_BYTES)
                    .await
            } else {
                None
            };
            (name.to_string(), importance, frame)
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
                return Ok(ObservationAnswer::ObservationRequiredCameraUnavailable {
                    camera: name,
                });
            }
            (CameraImportance::Optional, None) => missing_optional_cameras.push(name),
        }
    }
    missing_optional_cameras.sort();
    for arm in &state.arms {
        let link = links.get(&arm.arm).ok_or_else(state_unavailable)?;
        if link.interlocks().1 != arm.revision {
            return Ok(ObservationAnswer::ObservationRevisionChanged {
                arm: arm.arm.clone(),
            });
        }
    }
    Ok(ObservationAnswer::ObservationCaptured {
        state: Box::new(state),
        cameras,
        missing_optional_cameras,
    })
}

#[cfg(test)]
mod tests {
    use super::{ObservationRequest, ObservationSelection};
    use hebbian_contracts::SnapshotName;

    #[test]
    fn observation_cameras_must_be_unique_across_required_and_optional_lists() -> anyhow::Result<()>
    {
        let wrist = SnapshotName::try_from("wrist".to_owned())?;
        let selection = ObservationSelection::try_from(ObservationRequest {
            wait: None,
            arms: vec![],
            required_cameras: vec![wrist.clone()],
            optional_cameras: vec![wrist],
        });
        assert!(selection.is_err());
        Ok(())
    }

    #[test]
    fn observation_accepts_eight_cameras_total_and_rejects_nine() -> anyhow::Result<()> {
        let cameras = |indices: std::ops::Range<usize>| {
            indices
                .map(|index| SnapshotName::try_from(format!("camera_{index}")))
                .collect::<Result<Vec<_>, _>>()
        };
        let mut request = ObservationRequest {
            wait: None,
            arms: vec![],
            required_cameras: cameras(0..4)?,
            optional_cameras: cameras(4..8)?,
        };
        assert!(ObservationSelection::try_from(request.clone()).is_ok());
        request.optional_cameras.extend(cameras(8..9)?);
        assert!(ObservationSelection::try_from(request).is_err());
        Ok(())
    }
}

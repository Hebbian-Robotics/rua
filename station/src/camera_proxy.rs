use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use tokio::sync::Semaphore;

use crate::lease_routes::refusal;

const MAX_IN_FLIGHT: usize = 4;
const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const UPSTREAM_TIMEOUT: Duration = Duration::from_millis(750);
const CAPTURED_AT: &str = "x-captured-at-unix-ns";

struct CameraProxy {
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

pub(crate) fn router(media_address: Option<SocketAddr>) -> Router {
    let proxy = match media_address.map(CameraProxy::new).transpose() {
        Ok(proxy) => proxy.map(Arc::new),
        Err(error) => {
            tracing::warn!(%error, "Camera forwarding unavailable");
            None
        }
    };
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

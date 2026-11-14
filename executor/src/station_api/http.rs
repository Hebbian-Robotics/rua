//! Station HTTP routes, client sessions and event delivery.
//!
//! Commands cross [`crate::control_link`] to reach the controller. Routes,
//! preview workers, event replay and file serving run on the HTTP runtime,
//! outside the motor control cycle.
//!
//! Trace downloads are restricted to files directly inside the configured trace
//! directories. Viewer assets are read from disk at `/viewer/`, not embedded;
//! packaged installs provide them under the share directory or through
//! `$HEBBIAN_VIEWER_DIRECTORY`.

use hebbian_contracts::TraceFile;
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, UNIX_EPOCH};

use anyhow::Context;
use async_compression::tokio::bufread::GzipEncoder;
use axum::Json;
use axum::Router;
use axum::body::Body;
use axum::extract::{Path as UrlPath, Query, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use serde::Serialize;
use tokio::io::BufReader;
use tokio::sync::oneshot;
use tokio_util::io::ReaderStream;
use tower_http::services::ServeDir;
use tower_http::set_header::SetResponseHeader;

use crate::control_link::LinkHandle;

/// Threads for the HTTP runtime: enough for a few clients, none taken from the loop.
const WORKER_THREADS: usize = 2;
/// Bound concurrently active preview workers per HTTP request.
const PREVIEW_WORKERS: usize = 4;
/// How long open connections get to finish when the server closes.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);

#[derive(Serialize)]
struct Health {
    status: &'static str,
}

pub(crate) struct Served {
    trace_directories: Vec<PathBuf>,
    /// Each arm's link, by name, once the arms are set up; until then the
    /// HTTP routes report unavailable.
    pub(crate) arms: OnceLock<BTreeMap<String, LinkHandle>>,
    /// Where a model loop's live telemetry is kept; see `RunTelemetry`.
    pub(crate) telemetry: Arc<dyn crate::station_api::telemetry::RunTelemetry>,
    pub(crate) action_sequences: crate::station_api::action_sequences::ActionSequenceRegistry,
    pub(crate) instance_id: String,
    pub(crate) command_submissions: crate::station_api::submissions::CommandSubmissions,
    pub(crate) lease_submissions: crate::station_api::submissions::LeaseSubmissions,
    pub(crate) events: Arc<crate::station_api::events::EventJournal>,
}

/// A running HTTP server; `close` stops it.
pub struct HttpServer {
    served: Arc<Served>,
    runtime: Option<tokio::runtime::Runtime>,
    shutdown: Option<oneshot::Sender<()>>,
    address: SocketAddr,
}

impl HttpServer {
    /// Bind `address` and serve on new runtime threads, the viewer from
    /// `viewer_directory` when there is one. `telemetry` keeps what a model loop reports over HTTP.
    /// `extra_routes` is given the
    /// runtime and the bound address and returns the routes of whatever else
    /// this process serves, such as run telemetry and `LiveKit` access; those routes must
    /// never make the control path wait on the network.
    ///
    /// # Errors
    /// The runtime cannot start, the address cannot be bound, or `extra_routes` fails.
    pub fn start(
        address: &str,
        trace_directories: Vec<PathBuf>,
        viewer_directory: Option<PathBuf>,
        telemetry: Arc<dyn crate::station_api::telemetry::RunTelemetry>,
        extra_routes: impl FnOnce(&tokio::runtime::Handle, SocketAddr) -> anyhow::Result<Router>,
    ) -> anyhow::Result<Self> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(WORKER_THREADS)
            .thread_name("executor-http")
            .enable_all()
            .build()
            .context("Cannot start the HTTP runtime")?;
        let listener = runtime
            .block_on(tokio::net::TcpListener::bind(address))
            .with_context(|| format!("Cannot listen for HTTP on {address}"))?;
        let bound = listener.local_addr()?;
        let served = Arc::new(Served {
            trace_directories,
            arms: OnceLock::new(),
            telemetry,
            action_sequences: crate::station_api::action_sequences::ActionSequenceRegistry::default(
            ),
            instance_id: uuid::Uuid::new_v4().to_string(),
            command_submissions: crate::station_api::submissions::CommandSubmissions::default(),
            lease_submissions: crate::station_api::submissions::LeaseSubmissions::default(),
            events: Arc::new(crate::station_api::events::EventJournal::default()),
        });
        let extra = extra_routes(runtime.handle(), bound)?;
        let router = router(Arc::clone(&served), viewer_directory).merge(extra);
        let (shutdown, shutdown_requested) = oneshot::channel::<()>();
        runtime.spawn(async move {
            let served = axum::serve(listener, router).with_graceful_shutdown(async {
                let _ = shutdown_requested.await;
            });
            if let Err(error) = served.await {
                eprintln!("The HTTP server stopped: {error}");
            }
        });
        Ok(Self {
            served,
            runtime: Some(runtime),
            shutdown: Some(shutdown),
            address: bound,
        })
    }

    /// The address it listens on, with the port the system chose for port 0.
    #[must_use]
    pub fn local_address(&self) -> SocketAddr {
        self.address
    }

    /// The server's runtime, for work that belongs beside it.
    ///
    /// # Panics
    /// Never: the runtime lives until the server closes.
    #[must_use]
    pub fn runtime_handle(&self) -> tokio::runtime::Handle {
        self.runtime
            .as_ref()
            .expect("the runtime lives until the server closes")
            .handle()
            .clone()
    }

    /// Serve these arms over HTTP and follow their interlocks; only
    /// the first call counts.
    pub fn attach_arms(&self, arms: BTreeMap<String, LinkHandle>) {
        if self.served.arms.set(arms.clone()).is_ok()
            && let Some(runtime) = &self.runtime
        {
            let snapshot_arms = arms.clone();
            let snapshot_journal = Arc::clone(&self.served.events);
            runtime.spawn(async move {
                let mut interval = tokio::time::interval(Duration::from_millis(100));
                loop {
                    interval.tick().await;
                    if let Ok(Some(state)) = tokio::time::timeout(
                        Duration::from_secs(3),
                        crate::station_api::state::station_state(&snapshot_arms),
                    )
                    .await
                    {
                        snapshot_journal
                            .publish(serde_json::json!({"kind": "state", "state": state}));
                    } else {
                        snapshot_journal.invalidate_station_state();
                    }
                }
            });
            let sequence_station = Arc::clone(&self.served);
            runtime.spawn(async move {
                let mut interval = tokio::time::interval(Duration::from_millis(100));
                let mut previous = BTreeMap::new();
                loop {
                    interval.tick().await;
                    let records = sequence_station.action_sequences.snapshots();
                    let mut current = BTreeMap::new();
                    for record in records {
                        if previous.get(&record.id) != Some(&record) {
                            sequence_station.events.publish(serde_json::json!({"kind": "action_sequence_state", "action_sequence": record}));
                        }
                        current.insert(record.id.clone(), record);
                    }
                    previous = current;
                }
            });
            let telemetry = Arc::clone(&self.served.telemetry);
            let journal = Arc::clone(&self.served.events);
            runtime.spawn(async move {
                let mut cursor = telemetry.newest_seq();
                let mut revision = None;
                let mut interval = tokio::time::interval(Duration::from_millis(100));
                loop {
                    interval.tick().await;
                    let updates = telemetry.updates(&mut cursor, &mut revision);
                    for record in updates.records {
                        if let Ok(payload) = serde_json::to_value(record) {
                            journal.publish(payload);
                        }
                    }
                    if let Some(run) = updates.snapshot
                        && let Ok(payload) =
                            serde_json::to_value(hebbian_contracts::ServerMessage::Run {
                                run: Box::new(run),
                            })
                    {
                        journal.publish(payload);
                    }
                }
            });
            if let Some(lease_link) = arms.values().next().cloned() {
                runtime.spawn(async move {
                    let mut interval = tokio::time::interval(Duration::from_millis(100));
                    loop {
                        interval.tick().await;
                        let _ = lease_link.lease_status();
                    }
                });
            }
            for (arm_name, link) in arms {
                let journal = Arc::clone(&self.served.events);
                let event_link = link.clone();
                runtime.spawn(async move {
                    loop {
                        let (client, mut lines) = event_link.connect(4096);
                        event_link.snapshot(client);
                        while let Some(line) = lines.recv().await {
                            if let Ok(frame) = serde_json::from_str::<serde_json::Value>(&line) {
                                journal.publish(serde_json::json!({"kind": "frame", "arm": arm_name, "frame": frame}));
                            }
                        }
                        event_link.disconnect(client);
                        journal.publish(serde_json::json!({"kind": "resync_required", "arm": arm_name}));
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                });
                runtime.spawn(crate::station_api::interlocks::watch_arm(link));
            }
        }
    }

    pub fn close(mut self) {
        self.stop();
    }

    fn stop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_timeout(SHUTDOWN_GRACE);
        }
    }
}

impl Drop for HttpServer {
    fn drop(&mut self) {
        self.stop();
    }
}

fn router(served: Arc<Served>, viewer_directory: Option<PathBuf>) -> Router {
    let router = Router::new()
        .route("/health", get(|| async { Json(Health { status: "ok" }) }))
        .route("/instance", get(station_instance))
        .route("/events", get(stream_events))
        .route("/run/events", post(record_run_event))
        .route("/run/records", get(replay_run_records))
        .route("/run/spans", post(record_run_spans))
        .route("/commands", post(submit_command))
        .route("/commands/cancel", post(cancel_command_submission))
        .route("/group_commands", post(submit_group_command))
        .route("/traces", get(list_traces))
        .route("/traces/{name}", get(download_trace))
        .route("/pose_previews", post(preview_poses))
        .route("/leases", post(acquire_lease))
        .route("/leases/renew", post(renew_lease))
        .route("/leases/release", post(release_lease))
        .route("/leases/camera", post(report_lease_camera))
        .route("/arms/{arm}/stop", post(stop_leased_arm))
        .route("/action_sequence", post(start_action_sequence))
        .route("/action_sequence/{id}/cancel", post(cancel_action_sequence))
        .route("/state", get(read_state))
        .route("/action_sequence", get(read_action_sequence))
        .route("/action_sequence/{id}", get(read_action_sequence_by_id))
        .route("/commands/{id}", get(read_command))
        .with_state(served);
    match viewer_directory {
        Some(directory) => router
            .route("/", get(|| async { Redirect::temporary("/viewer/") }))
            .nest_service(
                "/viewer",
                // A rebuilt viewer keeps its file names, so revalidate each time.
                SetResponseHeader::overriding(
                    ServeDir::new(directory),
                    header::CACHE_CONTROL,
                    HeaderValue::from_static("no-cache"),
                ),
            ),
        None => router,
    }
}

fn arms_not_set_up() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        "The arms are not set up yet",
    )
        .into_response()
}

async fn read_state(State(served): State<Arc<Served>>) -> Response {
    let Some(arms) = served.arms.get() else {
        return arms_not_set_up();
    };
    match crate::station_api::state::station_state(arms).await {
        Some(state) => Json(state).into_response(),
        None => arms_not_set_up(),
    }
}

async fn read_action_sequence(State(served): State<Arc<Served>>) -> Response {
    let current = served
        .action_sequences
        .current()
        .map(|receiver| receiver.borrow().clone());
    Json(current).into_response()
}

async fn read_action_sequence_by_id(
    State(served): State<Arc<Served>>,
    UrlPath(id): UrlPath<String>,
) -> Response {
    match served.action_sequences.lookup(&id) {
        Some(action_sequence) => Json(action_sequence).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn read_command(
    State(served): State<Arc<Served>>,
    UrlPath(command_id): UrlPath<String>,
) -> Response {
    let Some(arms) = served.arms.get() else {
        return arms_not_set_up();
    };
    for (arm, link) in arms {
        if let Some(record) = link.find_record(command_id.clone()).await {
            return Json(hebbian_contracts::CommandLookup {
                arm: arm.clone(),
                record,
            })
            .into_response();
        }
    }
    (
        StatusCode::NOT_FOUND,
        format!("No arm remembers command {command_id}"),
    )
        .into_response()
}

async fn preview_poses(
    State(served): State<Arc<Served>>,
    Json(request): Json<hebbian_contracts::PosePreviewBatchRequest>,
) -> Result<Json<hebbian_contracts::PosePreviewBatchResponse>, (StatusCode, String)> {
    execute_pose_previews(served, request)
        .await
        .map(Json)
        .map_err(|error| {
            let status = match error {
                PosePreviewBatchError::Empty | PosePreviewBatchError::InvalidIdentifiers => {
                    StatusCode::BAD_REQUEST
                }
                PosePreviewBatchError::StationUnavailable => StatusCode::SERVICE_UNAVAILABLE,
                PosePreviewBatchError::WorkerFailed => StatusCode::INTERNAL_SERVER_ERROR,
            };
            (status, error.to_string())
        })
}

#[derive(Debug)]
pub(crate) enum PosePreviewBatchError {
    Empty,
    InvalidIdentifiers,
    StationUnavailable,
    WorkerFailed,
}

impl std::fmt::Display for PosePreviewBatchError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Empty => "Provide at least one pose",
            Self::InvalidIdentifiers => "Pose identifiers must be unique UUID v4 values",
            Self::StationUnavailable => "Station arms are not ready",
            Self::WorkerFailed => "Pose preview worker failed",
        })
    }
}

async fn start_action_sequence(
    State(served): State<Arc<Served>>,
    Json(input): Json<hebbian_contracts::ActionSequenceStartRequest>,
) -> Result<Json<String>, (axum::http::StatusCode, String)> {
    let links = served.arms.get().ok_or((
        axum::http::StatusCode::SERVICE_UNAVAILABLE,
        "Station arms are not ready".to_owned(),
    ))?;
    crate::station_api::action_sequences::start_action_sequence(
        &served,
        links,
        Some(input.lease_token),
        input.action_sequence,
    )
    .map(Json)
    .map_err(|detail| (axum::http::StatusCode::CONFLICT, detail))
}

async fn cancel_action_sequence(
    State(served): State<Arc<Served>>,
    UrlPath(id): UrlPath<String>,
) -> Result<axum::http::StatusCode, (axum::http::StatusCode, String)> {
    let links = served.arms.get().ok_or((
        axum::http::StatusCode::SERVICE_UNAVAILABLE,
        "Station arms are not ready".to_owned(),
    ))?;
    if crate::station_api::action_sequences::cancel_action_sequence(
        &served.action_sequences,
        links,
        &id,
    ) {
        Ok(axum::http::StatusCode::NO_CONTENT)
    } else {
        Err((
            axum::http::StatusCode::NOT_FOUND,
            "ActionSequence not found".to_owned(),
        ))
    }
}

async fn report_lease_camera(
    State(served): State<Arc<Served>>,
    Json(input): Json<hebbian_contracts::LeaseCameraReport>,
) -> Result<axum::http::StatusCode, (axum::http::StatusCode, String)> {
    let link = served
        .arms
        .get()
        .and_then(|arms| arms.values().next())
        .ok_or((
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "Station arms are not ready".to_owned(),
        ))?;
    link.report_camera(&input.lease_token, &input.camera, input.available)
        .map(|()| axum::http::StatusCode::NO_CONTENT)
        .map_err(|detail| {
            let still_holds = link
                .camera_requirements()
                .is_some_and(|requirements| requirements.token == input.lease_token);
            let status = if still_holds {
                axum::http::StatusCode::CONFLICT
            } else {
                axum::http::StatusCode::FORBIDDEN
            };
            (status, detail)
        })
}

async fn stop_leased_arm(
    State(served): State<Arc<Served>>,
    UrlPath(arm): UrlPath<String>,
    Json(input): Json<hebbian_contracts::LeaseToken>,
) -> Result<StatusCode, (StatusCode, String)> {
    let link = served
        .arms
        .get()
        .ok_or((
            StatusCode::SERVICE_UNAVAILABLE,
            "Station arms are not ready".to_owned(),
        ))?
        .get(&arm)
        .ok_or((StatusCode::NOT_FOUND, "Unknown arm".to_owned()))?;
    match link.stop_if_leased(Some(&input.lease_token)) {
        hebbian_contracts::LeasedStopOutcome::Submitted => Ok(StatusCode::NO_CONTENT),
        hebbian_contracts::LeasedStopOutcome::LeaseLost => {
            Err((StatusCode::FORBIDDEN, "Lease lost".to_owned()))
        }
        hebbian_contracts::LeasedStopOutcome::Unavailable => Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "Stop could not be submitted".to_owned(),
        )),
    }
}

async fn release_lease(
    State(served): State<Arc<Served>>,
    Json(input): Json<hebbian_contracts::LeaseToken>,
) -> Result<axum::http::StatusCode, (axum::http::StatusCode, String)> {
    let link = served
        .arms
        .get()
        .and_then(|arms| arms.values().next())
        .ok_or((
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "Station arms are not ready".to_owned(),
        ))?;
    if link.release_lease_with(&input.lease_token, || {
        served.action_sequences.cancel_owner(&input.lease_token)
    }) {
        Ok(axum::http::StatusCode::NO_CONTENT)
    } else {
        Err((
            axum::http::StatusCode::CONFLICT,
            "The lease has expired or is no longer held".to_owned(),
        ))
    }
}

async fn renew_lease(
    State(served): State<Arc<Served>>,
    Json(input): Json<hebbian_contracts::LeaseToken>,
) -> Result<axum::http::StatusCode, (axum::http::StatusCode, String)> {
    let link = served
        .arms
        .get()
        .and_then(|arms| arms.values().next())
        .ok_or((
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "Station arms are not ready".to_owned(),
        ))?;
    if link.retain_lease(&input.lease_token) {
        Ok(axum::http::StatusCode::NO_CONTENT)
    } else {
        Err((
            axum::http::StatusCode::CONFLICT,
            "The lease has expired or is no longer held".to_owned(),
        ))
    }
}

async fn wait_for_submission(
    mut response: tokio::sync::watch::Receiver<
        Option<Result<hebbian_contracts::CommandAnswer, String>>,
    >,
) -> Result<Json<hebbian_contracts::CommandAnswer>, (axum::http::StatusCode, String)> {
    loop {
        if let Some(outcome) = response.borrow_and_update().clone() {
            return outcome
                .map(Json)
                .map_err(|detail| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, detail));
        }
        response.changed().await.map_err(|_| {
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "Command outcome is unknown".to_owned(),
            )
        })?;
    }
}

fn validate_submission_id(
    served: &Served,
    station_instance_id: &str,
    request_id: &str,
) -> Result<uuid::Uuid, (axum::http::StatusCode, String)> {
    if station_instance_id != served.instance_id {
        return Err((
            axum::http::StatusCode::CONFLICT,
            "Station instance changed; request outcome is unknown".to_owned(),
        ));
    }
    let id = uuid::Uuid::parse_str(request_id).map_err(|_| {
        (
            axum::http::StatusCode::BAD_REQUEST,
            "Expected a UUID v4 request ID".to_owned(),
        )
    })?;
    if id.get_version_num() != 4 || id.get_variant() != uuid::Variant::RFC4122 {
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
            "Expected a UUID v4 request ID".to_owned(),
        ));
    }
    Ok(id)
}

async fn cancel_command_submission(
    State(served): State<Arc<Served>>,
    Json(input): Json<hebbian_contracts::CommandCancellationRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    use crate::station_api::submissions::CancellationResult;
    use hebbian_contracts::LeasedStopOutcome;

    let id = validate_submission_id(&served, &input.station_instance_id, &input.request_id)?;
    let arms = served.arms.get().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "Station arms are not ready".to_owned(),
    ))?;
    let unique_arms = input.arms.iter().collect::<std::collections::HashSet<_>>();
    if unique_arms.is_empty() || unique_arms.len() != input.arms.len() {
        return Err((StatusCode::BAD_REQUEST, "Expected unique arms".to_owned()));
    }
    let links = input
        .arms
        .iter()
        .map(|arm| arms.get(arm))
        .collect::<Option<Vec<_>>>()
        .ok_or((StatusCode::BAD_REQUEST, "Unknown arm".to_owned()))?;
    let result = served
        .command_submissions
        .cancel(id, &input.lease_token, &input.arms, || {
            let mut result = LeasedStopOutcome::Submitted;
            for link in links {
                match link.stop_if_leased(Some(&input.lease_token)) {
                    LeasedStopOutcome::Submitted => {}
                    LeasedStopOutcome::LeaseLost => return LeasedStopOutcome::LeaseLost,
                    LeasedStopOutcome::Unavailable => result = LeasedStopOutcome::Unavailable,
                }
            }
            result
        });
    match result {
        CancellationResult::Stopped(LeasedStopOutcome::Submitted) => Ok(StatusCode::NO_CONTENT),
        CancellationResult::Stopped(LeasedStopOutcome::LeaseLost) => {
            Err((StatusCode::CONFLICT, "Lease lost".to_owned()))
        }
        CancellationResult::Stopped(LeasedStopOutcome::Unavailable) => Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "Stop could not be submitted".to_owned(),
        )),
        CancellationResult::Conflict => Err((
            StatusCode::CONFLICT,
            "Cancellation does not match the submission".to_owned(),
        )),
    }
}

async fn submit_group_command(
    State(served): State<Arc<Served>>,
    Json(input): Json<hebbian_contracts::GroupCommandSubmissionRequest>,
) -> Result<Json<hebbian_contracts::CommandAnswer>, (axum::http::StatusCode, String)> {
    use crate::station_api::submissions::{SubmissionRequest, SubmissionReservation};
    let id = validate_submission_id(&served, &input.station_instance_id, &input.request_id)?;
    let links = served
        .arms
        .get()
        .ok_or((
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "Station arms are not ready".to_owned(),
        ))?
        .clone();
    let generation = input
        .command
        .targets
        .keys()
        .filter_map(|arm| links.get(arm))
        .map(crate::control_link::LinkHandle::submission_generation)
        .collect::<Vec<_>>();
    let response = match served
        .command_submissions
        .reserve(id, SubmissionRequest::Group(input.clone()))
    {
        SubmissionReservation::Conflict => {
            return Err((
                axum::http::StatusCode::CONFLICT,
                "Request ID already used for a different command".to_owned(),
            ));
        }
        SubmissionReservation::Expired => {
            return Err((
                axum::http::StatusCode::GONE,
                "Request outcome is no longer retained".to_owned(),
            ));
        }
        SubmissionReservation::Full => {
            return Err((
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "Too many command submissions are still in flight".to_owned(),
            ));
        }
        SubmissionReservation::Existing(response) => response,
        SubmissionReservation::Cancelled => {
            return Ok(Json(hebbian_contracts::CommandAnswer::Refused {
                reason: hebbian_contracts::RejectionReason::ArmBusy,
                detail: "Command submission was cancelled".to_owned(),
            }));
        }
        SubmissionReservation::New(sender) => {
            let response = sender.subscribe();
            let station = Arc::clone(&served);
            tokio::spawn(async move {
                let admitted = tokio::task::spawn_blocking(move || {
                    let token = input.lease_token.as_deref();
                    let targets = input
                        .command
                        .targets
                        .keys()
                        .map(String::as_str)
                        .collect::<Vec<_>>();
                    station
                        .action_sequences
                        .check_command_conflict(token, &targets)
                        .map_err(|detail| {
                            (
                                hebbian_contracts::RejectionReason::ArmBusy,
                                detail.to_owned(),
                            )
                        })?;
                    let (link, moves, gates) = crate::station_api::commands::plan_group_move(
                        &links,
                        &input.command,
                        &input.expected_revisions,
                    )?;
                    let arms = moves
                        .iter()
                        .map(|part| part.arm.clone())
                        .collect::<Vec<_>>();
                    let reply = link
                        .move_arms_async(moves, token, input.trace_context, &gates, &generation)
                        .map_err(|error| (error.reason, error.detail))?;
                    Ok::<_, (hebbian_contracts::RejectionReason, String)>((arms, reply))
                })
                .await;
                let outcome = match admitted {
                    Err(_) => Err("Command worker failed; outcome is unknown".to_owned()),
                    Ok(Err((reason, detail))) => {
                        Ok(hebbian_contracts::CommandAnswer::Refused { reason, detail })
                    }
                    Ok(Ok((arms, reply))) => match reply.await {
                        Ok(Ok(records)) if arms.len() == records.len() => {
                            Ok(hebbian_contracts::CommandAnswer::Accepted {
                                records: arms
                                    .into_iter()
                                    .zip(records)
                                    .map(|(arm, record)| hebbian_contracts::ArmRecord {
                                        arm,
                                        record,
                                    })
                                    .collect(),
                            })
                        }
                        Ok(Ok(_)) => {
                            Err("Control loop returned an unexpected number of records".to_owned())
                        }
                        Ok(Err(error)) => Ok(hebbian_contracts::CommandAnswer::Refused {
                            reason: error.reason,
                            detail: error.detail,
                        }),
                        Err(_) => {
                            Err("Control loop reply lost; command outcome is unknown".to_owned())
                        }
                    },
                };
                sender.send_replace(Some(outcome));
            });
            response
        }
    };
    wait_for_submission(response).await
}

async fn submit_command(
    State(served): State<Arc<Served>>,
    Json(input): Json<hebbian_contracts::CommandSubmissionRequest>,
) -> Result<Json<hebbian_contracts::CommandAnswer>, (axum::http::StatusCode, String)> {
    use crate::station_api::submissions::SubmissionReservation;
    let id = validate_submission_id(&served, &input.station_instance_id, &input.request_id)?;
    let links = served
        .arms
        .get()
        .ok_or((
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "Station arms are not ready".to_owned(),
        ))?
        .clone();
    let generation = links
        .get(&input.arm)
        .map(crate::control_link::LinkHandle::submission_generation)
        .into_iter()
        .collect::<Vec<_>>();
    let response = match served.command_submissions.reserve(
        id,
        crate::station_api::submissions::SubmissionRequest::Single(input.clone()),
    ) {
        SubmissionReservation::Conflict => {
            return Err((
                axum::http::StatusCode::CONFLICT,
                "Request ID already used for a different command".to_owned(),
            ));
        }
        SubmissionReservation::Expired => {
            return Err((
                axum::http::StatusCode::GONE,
                "Request outcome is no longer retained".to_owned(),
            ));
        }
        SubmissionReservation::Full => {
            return Err((
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "Too many command submissions are still in flight".to_owned(),
            ));
        }
        SubmissionReservation::Existing(response) => response,
        SubmissionReservation::Cancelled => {
            return Ok(Json(hebbian_contracts::CommandAnswer::Refused {
                reason: hebbian_contracts::RejectionReason::ArmBusy,
                detail: "Command submission was cancelled".to_owned(),
            }));
        }
        SubmissionReservation::New(sender) => {
            let response = sender.subscribe();
            let station = Arc::clone(&served);
            tokio::spawn(async move {
                let arm = input.arm.clone();
                let admitted = tokio::task::spawn_blocking(move || {
                    let mut generation = generation;
                    let token = input.lease_token.as_deref();
                    let link = links.get(&input.arm).ok_or((
                        hebbian_contracts::RejectionReason::InvalidTarget,
                        "Unknown arm".to_owned(),
                    ))?;
                    if matches!(input.command, hebbian_contracts::StationCommand::Stop) {
                        return link
                            .interrupt_async(input.trace_context)
                            .map_err(|error| (error.reason, error.detail));
                    }
                    if matches!(input.command, hebbian_contracts::StationCommand::Disable) {
                        if let Some(token) = token {
                            link.stop_leased_motion_for_submission(token, &mut generation, || {
                                station.action_sequences.cancel_owner(token)
                            })
                            .map_err(|error| (error.reason, error.detail))?;
                        }
                    } else if let Err(detail) = station
                        .action_sequences
                        .check_command_conflict(token, &[&input.arm])
                    {
                        return Err((
                            hebbian_contracts::RejectionReason::ArmBusy,
                            detail.to_owned(),
                        ));
                    }
                    let command =
                        crate::station_api::commands::arm_command(link, &input.arm, input.command)?;
                    link.submit_command_async(
                        command,
                        token,
                        input.trace_context,
                        input.expected_revision,
                        &generation,
                    )
                    .map_err(|error| (error.reason, error.detail))
                })
                .await;
                let outcome = match admitted {
                    Err(_) => Err("Command worker failed; outcome is unknown".to_owned()),
                    Ok(Err((reason, detail))) => {
                        Ok(hebbian_contracts::CommandAnswer::Refused { reason, detail })
                    }
                    Ok(Ok(reply)) => match reply.await {
                        Ok(Ok(record)) => Ok(hebbian_contracts::CommandAnswer::Accepted {
                            records: vec![hebbian_contracts::ArmRecord { arm, record }],
                        }),
                        Ok(Err(error)) => Ok(hebbian_contracts::CommandAnswer::Refused {
                            reason: error.reason,
                            detail: error.detail,
                        }),
                        Err(_) => {
                            Err("Control loop reply lost; command outcome is unknown".to_owned())
                        }
                    },
                };
                sender.send_replace(Some(outcome));
            });
            response
        }
    };
    wait_for_submission(response).await
}

async fn record_run_event(
    State(served): State<Arc<Served>>,
    Json(event): Json<hebbian_contracts::RunEvent>,
) -> axum::http::StatusCode {
    served.telemetry.record_event(event);
    axum::http::StatusCode::NO_CONTENT
}

#[derive(serde::Deserialize)]
struct RunReplayQuery {
    after: u64,
}

async fn replay_run_records(
    State(served): State<Arc<Served>>,
    Query(query): Query<RunReplayQuery>,
) -> Json<Vec<hebbian_contracts::ServerMessage>> {
    let mut cursor = query.after;
    let updates = served.telemetry.updates(&mut cursor, &mut None);
    Json(updates.records)
}

async fn record_run_spans(
    State(served): State<Arc<Served>>,
    Json(request): Json<hebbian_contracts::RunSpansRequest>,
) -> axum::http::StatusCode {
    served.telemetry.record_spans(request.spans);
    axum::http::StatusCode::NO_CONTENT
}

async fn stream_events(
    State(served): State<Arc<Served>>,
    headers: axum::http::HeaderMap,
) -> Result<impl axum::response::IntoResponse, (axum::http::StatusCode, String)> {
    use crate::station_api::events::EventSubscription;
    use axum::response::sse::Event;
    let cursor = headers
        .get("last-event-id")
        .map(|value| {
            value
                .to_str()
                .ok()
                .and_then(|value| value.rsplit_once(':'))
                .and_then(|(instance, sequence)| {
                    sequence
                        .parse::<u64>()
                        .ok()
                        .map(|sequence| (instance.to_owned(), sequence))
                })
        })
        .map_or(Ok(None), |parsed| {
            parsed.map(Some).ok_or((
                axum::http::StatusCode::BAD_REQUEST,
                "Invalid event cursor".to_owned(),
            ))
        })?;
    let (initial, mut updates) = match cursor {
        None => {
            let (sequence, snapshot, updates) = served.events.subscribe_with_snapshot();
            if !snapshot
                .iter()
                .any(|event| event.get("kind").and_then(serde_json::Value::as_str) == Some("state"))
            {
                return Err((
                    axum::http::StatusCode::SERVICE_UNAVAILABLE,
                    "Station snapshot is not ready".to_owned(),
                ));
            }
            let event = Event::default().event("snapshot").id(format!("{}:{sequence}", served.instance_id)).data(serde_json::json!({"station_instance_id": served.instance_id, "sequence": sequence, "run_sequence": served.telemetry.newest_seq(), "items": snapshot}).to_string());
            (vec![event], updates)
        }
        Some((instance, sequence)) if instance == served.instance_id => {
            match served.events.subscribe_after(sequence) {
                EventSubscription::Replay { events, updates } => (
                    events
                        .into_iter()
                        .map(|event| {
                            Event::default()
                                .event("station")
                                .id(format!("{}:{}", served.instance_id, event.sequence))
                                .data(event.payload.to_string())
                        })
                        .collect(),
                    updates,
                ),
                EventSubscription::ResyncRequired => {
                    return Err((
                        axum::http::StatusCode::CONFLICT,
                        "Event cursor is no longer retained; request a new snapshot".to_owned(),
                    ));
                }
            }
        }
        Some(_) => {
            return Err((
                axum::http::StatusCode::CONFLICT,
                "Station instance changed; request a new snapshot".to_owned(),
            ));
        }
    };
    let (sender, receiver) =
        tokio::sync::mpsc::channel::<Result<Event, std::convert::Infallible>>(64);
    let instance_id = served.instance_id.clone();
    tokio::spawn(async move {
        for event in initial {
            if sender.send(Ok(event)).await.is_err() {
                return;
            }
        }
        loop {
            match updates.recv().await {
                Ok(event) => {
                    let event = Event::default()
                        .event("station")
                        .id(format!("{instance_id}:{}", event.sequence))
                        .data(event.payload.to_string());
                    if sender.send(Ok(event)).await.is_err() {
                        return;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    let _ = sender
                        .send(Ok(Event::default()
                            .event("resync_required")
                            .data("Event stream fell behind")))
                        .await;
                    return;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
            }
        }
    });
    Ok(
        axum::response::sse::Sse::new(tokio_stream::wrappers::ReceiverStream::new(receiver))
            .keep_alive(axum::response::sse::KeepAlive::default()),
    )
}

async fn station_instance(
    State(served): State<Arc<Served>>,
) -> Json<hebbian_contracts::StationInstance> {
    Json(hebbian_contracts::StationInstance {
        id: served.instance_id.clone(),
    })
}

async fn acquire_lease(
    State(served): State<Arc<Served>>,
    Json(input): Json<hebbian_contracts::LeaseAcquireRequest>,
) -> Result<Json<hebbian_contracts::LeaseGrant>, (StatusCode, String)> {
    use crate::station_api::submissions::LeaseReservation;

    let id = validate_submission_id(&served, &input.station_instance_id, &input.request_id)?;
    let links = served
        .arms
        .get()
        .ok_or((
            StatusCode::SERVICE_UNAVAILABLE,
            "Station arms are not ready".to_owned(),
        ))?
        .values()
        .cloned()
        .collect::<Vec<_>>();
    let link = links.first().cloned().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "Station arms are not ready".to_owned(),
    ))?;
    let mut response = match served.lease_submissions.reserve(id, input.clone()) {
        LeaseReservation::Conflict => {
            return Err((
                StatusCode::CONFLICT,
                "Request ID already used for a different lease request".to_owned(),
            ));
        }
        LeaseReservation::Expired => {
            return Err((
                StatusCode::GONE,
                "Lease request outcome is no longer retained".to_owned(),
            ));
        }
        LeaseReservation::Full => {
            return Err((
                StatusCode::SERVICE_UNAVAILABLE,
                "Too many lease acquisitions are still in flight".to_owned(),
            ));
        }
        LeaseReservation::Existing(response) => response,
        LeaseReservation::New(sender) => {
            let response = sender.subscribe();
            tokio::spawn(async move {
                let result = tokio::task::spawn_blocking(move || {
                    if input.request.holder == hebbian_contracts::LeaseHolder::Manual {
                        crate::lease::manual_control_ready_without_waiting(&links)?;
                    }
                    link.acquire_lease(input.request, input.take)
                        .map_err(|error| error.to_string())
                })
                .await
                .unwrap_or_else(|_| Err("Lease worker failed; outcome is unknown".to_owned()));
                sender.send_replace(Some(result));
            });
            response
        }
    };
    loop {
        if let Some(outcome) = response.borrow_and_update().clone() {
            return outcome
                .map(Json)
                .map_err(|detail| (StatusCode::CONFLICT, detail));
        }
        response.changed().await.map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Lease outcome is unknown".to_owned(),
            )
        })?;
    }
}

pub(crate) async fn execute_pose_previews(
    served: Arc<Served>,
    request: hebbian_contracts::PosePreviewBatchRequest,
) -> Result<hebbian_contracts::PosePreviewBatchResponse, PosePreviewBatchError> {
    let mut identifiers = std::collections::HashSet::new();
    if request.poses.is_empty() {
        return Err(PosePreviewBatchError::Empty);
    }
    for candidate in &request.poses {
        let identifier = uuid::Uuid::parse_str(&candidate.id)
            .map_err(|_| PosePreviewBatchError::InvalidIdentifiers)?;
        if identifier.get_version_num() != 4
            || identifier.get_variant() != uuid::Variant::RFC4122
            || !identifiers.insert(identifier)
        {
            return Err(PosePreviewBatchError::InvalidIdentifiers);
        }
    }
    let arms = served
        .arms
        .get()
        .ok_or(PosePreviewBatchError::StationUnavailable)?
        .clone();
    let mut pending = request.poses.into_iter().enumerate();
    let mut workers = tokio::task::JoinSet::new();
    let mut results = Vec::new();
    loop {
        while workers.len() < PREVIEW_WORKERS {
            let Some((index, candidate)) = pending.next() else {
                break;
            };
            let arms = arms.clone();
            let arm = request.arm.clone();
            workers.spawn_blocking(move || {
                let answer =
                    crate::station_api::commands::preview_answer(&arms, &arm, candidate.pose);
                (
                    index,
                    hebbian_contracts::PosePreviewCandidateResult {
                        id: candidate.id,
                        label: candidate.label,
                        answer,
                    },
                )
            });
        }
        let Some(result) = workers.join_next().await else {
            break;
        };
        results.push(result.map_err(|_| PosePreviewBatchError::WorkerFailed)?);
    }
    results.sort_unstable_by_key(|(index, _)| *index);
    Ok(hebbian_contracts::PosePreviewBatchResponse {
        results: results.into_iter().map(|(_, result)| result).collect(),
    })
}

/// Overrides where the built viewer is read from.
pub const VIEWER_DIRECTORY_VARIABLE: &str = "HEBBIAN_VIEWER_DIRECTORY";

/// Where the built viewer is: `$HEBBIAN_VIEWER_DIRECTORY`, else the nearest
/// installed `share/hebbian-robot-use/dist/viewer` or a checkout's `dist/viewer`
/// above the running binary or working directory; none when no build exists.
#[must_use]
pub fn default_viewer_directory() -> Option<PathBuf> {
    if let Some(directory) =
        std::env::var_os(VIEWER_DIRECTORY_VARIABLE).filter(|value| !value.is_empty())
    {
        return Some(PathBuf::from(directory));
    }
    let starts = std::env::current_exe()
        .into_iter()
        .chain(std::env::current_dir());
    starts
        .flat_map(|start| start.ancestors().map(Path::to_owned).collect::<Vec<_>>())
        .flat_map(|ancestor| {
            [
                ancestor.join("share/hebbian-robot-use/dist/viewer"),
                ancestor.join("dist/viewer"),
            ]
        })
        .find(|candidate| candidate.join("index.html").is_file())
}

/// Every regular file directly inside the trace directories, newest first.
fn trace_files(directories: &[PathBuf]) -> Vec<(TraceFile, PathBuf)> {
    let mut files = Vec::new();
    for directory in directories {
        let Ok(entries) = std::fs::read_dir(directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if !metadata.is_file() || name.starts_with('.') {
                continue;
            }
            let modified_unix_seconds = metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map_or(0.0, |since| since.as_secs_f64());
            files.push((
                TraceFile {
                    name,
                    size_bytes: metadata.len(),
                    modified_unix_seconds,
                },
                entry.path(),
            ));
        }
    }
    files.sort_by(|left, right| {
        right
            .0
            .modified_unix_seconds
            .total_cmp(&left.0.modified_unix_seconds)
    });
    files
}

async fn list_traces(State(served): State<Arc<Served>>) -> Response {
    let directories = served.trace_directories.clone();
    match tokio::task::spawn_blocking(move || trace_files(&directories)).await {
        Ok(files) => {
            Json(files.into_iter().map(|(file, _)| file).collect::<Vec<_>>()).into_response()
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// The trace called `name`, found by listing, never by joining the name to a path.
fn find_trace(directories: &[PathBuf], name: &str) -> Option<PathBuf> {
    trace_files(directories)
        .into_iter()
        .find(|(file, _)| file.name == name)
        .map(|(_, path)| path)
}

async fn download_trace(
    State(served): State<Arc<Served>>,
    UrlPath(name): UrlPath<String>,
) -> Response {
    let directories = served.trace_directories.clone();
    let lookup_name = name.clone();
    let Ok(Some(path)) =
        tokio::task::spawn_blocking(move || find_trace(&directories, &lookup_name)).await
    else {
        return (StatusCode::NOT_FOUND, format!("No trace named {name}")).into_response();
    };
    match tokio::fs::File::open(&path).await {
        Ok(file) => {
            let compressed = GzipEncoder::new(BufReader::new(file));
            (
                [
                    (header::CONTENT_TYPE, "application/gzip".to_owned()),
                    (
                        header::CONTENT_DISPOSITION,
                        format!("attachment; filename=\"{name}.gz\""),
                    ),
                ],
                Body::from_stream(ReaderStream::new(compressed)),
            )
                .into_response()
        }
        Err(error) => (StatusCode::NOT_FOUND, error.to_string()).into_response(),
    }
}

/// The distinct trace directories of these arms' configs.
#[must_use]
pub fn trace_directories<'a>(directories: impl IntoIterator<Item = &'a Path>) -> Vec<PathBuf> {
    let mut distinct: Vec<PathBuf> = Vec::new();
    for directory in directories {
        if !distinct.iter().any(|known| known == directory) {
            distinct.push(directory.to_owned());
        }
    }
    distinct
}

#[cfg(test)]
mod event_stream_tests {
    use super::{HttpServer, Served, stream_events};
    use axum::extract::State;
    use axum::http::{HeaderMap, HeaderValue, StatusCode};
    use std::sync::{Arc, OnceLock};

    fn station() -> Arc<Served> {
        Arc::new(Served {
            trace_directories: vec![],
            arms: OnceLock::new(),
            telemetry: Arc::new(crate::station_api::telemetry::DiscardTelemetry),
            action_sequences: crate::station_api::action_sequences::ActionSequenceRegistry::default(
            ),
            instance_id: "instance-a".into(),
            command_submissions: crate::station_api::submissions::CommandSubmissions::default(),
            lease_submissions: crate::station_api::submissions::LeaseSubmissions::default(),
            events: Arc::new(crate::station_api::events::EventJournal::default()),
        })
    }

    #[test]
    fn http_stream_resumes_after_the_delivered_event() {
        let server = HttpServer::start(
            "127.0.0.1:0",
            vec![],
            None,
            Arc::new(crate::station_api::telemetry::DiscardTelemetry),
            |_, _| Ok(axum::Router::new()),
        )
        .unwrap();
        server
            .served
            .events
            .publish(serde_json::json!({"kind": "state", "state": {}}));
        let url = format!("http://{}/events", server.local_address());
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            async fn next_event(response: &mut reqwest::Response) -> String {
                let mut buffer = Vec::new();
                loop {
                    let chunk =
                        tokio::time::timeout(std::time::Duration::from_secs(2), response.chunk())
                            .await
                            .unwrap()
                            .unwrap()
                            .unwrap();
                    buffer.extend_from_slice(&chunk);
                    if buffer.windows(2).any(|window| window == b"\n\n") {
                        return String::from_utf8(buffer).unwrap();
                    }
                }
            }
            let client = reqwest::Client::new();
            let mut response = client
                .get(&url)
                .send()
                .await
                .unwrap()
                .error_for_status()
                .unwrap();
            let snapshot = next_event(&mut response).await;
            assert!(snapshot.contains("event: snapshot"));
            let cursor = snapshot
                .lines()
                .find_map(|line| line.strip_prefix("id: "))
                .unwrap();
            let cursor = cursor.to_owned();
            drop(response);
            server
                .served
                .events
                .publish(serde_json::json!({"kind": "run", "test_marker": "replayed"}));
            let mut resumed = client
                .get(&url)
                .header("Last-Event-ID", cursor)
                .send()
                .await
                .unwrap()
                .error_for_status()
                .unwrap();
            let replay = next_event(&mut resumed).await;
            assert!(replay.contains("event: station"));
            assert!(replay.contains("replayed"));
        });
    }

    #[tokio::test]
    async fn initial_stream_requires_state_and_old_instances_require_resync() {
        let served = station();
        let error = stream_events(State(Arc::clone(&served)), HeaderMap::new())
            .await
            .err()
            .unwrap();
        assert_eq!(error.0, StatusCode::SERVICE_UNAVAILABLE);
        served
            .events
            .publish(serde_json::json!({"kind":"state", "state":{}}));
        assert!(
            stream_events(State(Arc::clone(&served)), HeaderMap::new())
                .await
                .is_ok()
        );
        served.events.invalidate_station_state();
        let error = stream_events(State(Arc::clone(&served)), HeaderMap::new())
            .await
            .err()
            .unwrap();
        assert_eq!(error.0, StatusCode::SERVICE_UNAVAILABLE);
        let mut headers = HeaderMap::new();
        headers.insert("last-event-id", HeaderValue::from_static("old-instance:1"));
        let error = stream_events(State(served), headers).await.err().unwrap();
        assert_eq!(error.0, StatusCode::CONFLICT);
    }
}

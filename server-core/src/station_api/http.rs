use hebbian_contracts::{LeasedStopOutcome, ServerMessage, TraceFile};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, UNIX_EPOCH};

use anyhow::Context;
use async_compression::tokio::bufread::GzipEncoder;
use axum::Json;
use axum::Router;
use axum::body::Body;
use axum::extract::{Path as UrlPath, Query, Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use serde::Serialize;
use tokio::io::BufReader;
use tokio::sync::oneshot;
use tokio_util::io::ReaderStream;
use tower_http::services::ServeDir;
use tower_http::set_header::SetResponseHeader;

use crate::control_link::LinkHandle;

use super::submissions::{SubmissionCompletion, wait_for_outcome};

const WORKER_THREADS: usize = 2;
const PREVIEW_WORKERS: usize = 4;
pub(super) const SHUTTING_DOWN: &str = "Station is shutting down";
const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);

#[derive(Serialize)]
struct Health {
    status: &'static str,
}

pub(crate) struct StationState {
    shutdown_requested: Arc<AtomicBool>,
    pub(crate) program_submissions: super::submissions::ProgramSubmissions,
    pub(crate) compiler: crate::station_api::programs::ProgramStore,
    compilation_targets:
        OnceLock<Result<BTreeMap<String, hebbian_contracts::CompilationTarget>, String>>,
    trace_directories: Vec<PathBuf>,
    pub(crate) arms: OnceLock<BTreeMap<String, LinkHandle>>,
    pub(crate) telemetry: Arc<dyn crate::station_api::telemetry::RunTelemetry>,
    pub(crate) executions: crate::station_api::runtime::ExecutionRegistry,
    pub(crate) instance_id: String,
    pub(crate) command_submissions: crate::station_api::submissions::CommandSubmissions,
    pub(crate) lease_submissions: crate::station_api::submissions::LeaseSubmissions,
    pub(crate) events: Arc<crate::station_api::events::EventJournal>,
}

impl StationState {
    pub(super) fn is_shutting_down(&self) -> bool {
        self.shutdown_requested.load(Ordering::Acquire)
    }

    pub(crate) fn compilation_targets(
        &self,
    ) -> Result<&BTreeMap<String, hebbian_contracts::CompilationTarget>, String> {
        let arms = self.arms.get().ok_or("Station arms are not ready")?;
        self.compilation_targets
            .get_or_init(|| {
                super::programs::station_targets(arms).map_err(|error| error.to_string())
            })
            .as_ref()
            .map_err(Clone::clone)
    }

    // INVARIANT: Every arm shares the station lease; any ready arm can handle lease operations.
    fn lease_link(&self) -> Result<&LinkHandle, (StatusCode, String)> {
        self.arms
            .get()
            .and_then(|arms| arms.values().next())
            .ok_or((
                StatusCode::SERVICE_UNAVAILABLE,
                "Station arms are not ready".to_owned(),
            ))
    }
}

pub struct HttpServer {
    station_state: Arc<StationState>,
    runtime: Option<tokio::runtime::Runtime>,
    shutdown: Option<oneshot::Sender<()>>,
    address: SocketAddr,
}

impl HttpServer {
    // CONSTRAINT: Extra HTTP routes run outside the motor loop and must never make the control path wait on network I/O.
    pub fn start(
        address: &str,
        trace_directories: Vec<PathBuf>,
        viewer_directory: Option<PathBuf>,
        telemetry: Arc<dyn crate::station_api::telemetry::RunTelemetry>,
        shutdown_requested: Arc<AtomicBool>,
        extra_routes: impl FnOnce(&tokio::runtime::Handle, SocketAddr) -> anyhow::Result<Router>,
    ) -> anyhow::Result<Self> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(WORKER_THREADS)
            .thread_name("station-http")
            .enable_all()
            .build()
            .context("Cannot start the HTTP runtime")?;
        let listener = runtime
            .block_on(tokio::net::TcpListener::bind(address))
            .with_context(|| format!("Cannot listen for HTTP on {address}"))?;
        let bound = listener.local_addr()?;
        let station_state = Arc::new(StationState {
            shutdown_requested,
            program_submissions: crate::station_api::submissions::ProgramSubmissions::default(),
            compiler: crate::station_api::programs::ProgramStore::default(),
            compilation_targets: OnceLock::new(),
            trace_directories,
            arms: OnceLock::new(),
            telemetry,
            executions: crate::station_api::runtime::ExecutionRegistry::default(),
            instance_id: uuid::Uuid::new_v4().to_string(),
            command_submissions: crate::station_api::submissions::CommandSubmissions::default(),
            lease_submissions: crate::station_api::submissions::LeaseSubmissions::default(),
            events: Arc::new(crate::station_api::events::EventJournal::default()),
        });
        let extra = extra_routes(runtime.handle(), bound)?;
        let router = router(Arc::clone(&station_state), viewer_directory).merge(extra);
        let (shutdown, shutdown_requested) = oneshot::channel::<()>();
        runtime.spawn(async move {
            let http_server = axum::serve(listener, router).with_graceful_shutdown(async {
                let _ = shutdown_requested.await;
            });
            if let Err(error) = http_server.await {
                eprintln!("The HTTP server stopped: {error}");
            }
        });
        Ok(Self {
            station_state,
            runtime: Some(runtime),
            shutdown: Some(shutdown),
            address: bound,
        })
    }

    #[must_use]
    pub fn local_address(&self) -> SocketAddr {
        self.address
    }

    #[must_use]
    #[expect(
        clippy::expect_used,
        reason = "Only consuming close or Drop removes the runtime, so every borrowed server still has one."
    )]
    pub fn runtime_handle(&self) -> tokio::runtime::Handle {
        self.runtime
            .as_ref()
            .expect("the runtime lives until the server closes")
            .handle()
            .clone()
    }

    pub fn attach_arms(&self, arms: BTreeMap<String, LinkHandle>) {
        if self.station_state.arms.set(arms.clone()).is_err() {
            return;
        }
        let Some(runtime) = &self.runtime else {
            return;
        };
        let snapshot_arms = arms.clone();
        let snapshot_journal = Arc::clone(&self.station_state.events);
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
                    snapshot_journal.publish(ServerMessage::State {
                        state: Box::new(state),
                    });
                } else {
                    snapshot_journal.invalidate_station_state();
                }
            }
        });
        let sequence_station = Arc::clone(&self.station_state);
        runtime.spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(100));
            let mut previous = BTreeMap::new();
            loop {
                interval.tick().await;
                let records = sequence_station.executions.snapshots();
                let mut current = BTreeMap::new();
                for record in records {
                    if previous.get(&record.id) != Some(&record) {
                        sequence_station
                            .events
                            .publish(ServerMessage::ActionSequenceState {
                                action_sequence: Some(record.clone()),
                            });
                    }
                    current.insert(record.id.clone(), record);
                }
                previous = current;
            }
        });
        let telemetry = Arc::clone(&self.station_state.telemetry);
        let journal = Arc::clone(&self.station_state.events);
        runtime.spawn(async move {
            let mut cursor = telemetry.newest_seq();
            let mut revision = None;
            let mut interval = tokio::time::interval(Duration::from_millis(100));
            loop {
                interval.tick().await;
                let updates = telemetry.updates(&mut cursor, &mut revision);
                for record in updates.records {
                    journal.publish(record);
                }
                if let Some(run) = updates.snapshot {
                    journal.publish(ServerMessage::Run { run: Box::new(run) });
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
            let journal = Arc::clone(&self.station_state.events);
            let event_link = link.clone();
            runtime.spawn(async move {
                loop {
                    let (client, mut frames) = event_link.connect(4096);
                    event_link.snapshot(client);
                    while let Some(frame) = frames.recv().await {
                        journal.publish(ServerMessage::Frame {
                            arm: arm_name.clone(),
                            frame: Box::new(frame),
                        });
                    }
                    event_link.disconnect(client);
                    journal.resync_arm(arm_name.clone());
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            });
            runtime.spawn(crate::station_api::interlocks::watch_arm(link));
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

async fn reject_while_shutting_down(
    State(station_state): State<Arc<StationState>>,
    request: Request,
    next: Next,
) -> Response {
    if station_state.is_shutting_down() {
        return (StatusCode::SERVICE_UNAVAILABLE, SHUTTING_DOWN).into_response();
    }
    next.run(request).await
}

fn router(station_state: Arc<StationState>, viewer_directory: Option<PathBuf>) -> Router {
    let admission_routes = Router::new()
        .route("/commands", post(submit_command))
        .route("/group_commands", post(submit_group_command))
        .route("/pose_previews", post(preview_poses))
        .route("/leases", post(acquire_lease))
        .route("/leases/renew", post(renew_lease))
        .route(
            "/compilations/compile",
            post(super::program_http::compile_action_sequence),
        )
        .route(
            "/compilations/import",
            post(super::program_http::import_compilation),
        )
        .route_layer(middleware::from_fn_with_state(
            Arc::clone(&station_state),
            reject_while_shutting_down,
        ));
    let router = Router::new()
        .merge(admission_routes)
        .route("/health", get(|| async { Json(Health { status: "ok" }) }))
        .route(
            "/model/{arm}/{*asset}",
            get(super::model_assets::read_model_asset),
        )
        .route("/instance", get(station_instance))
        .route("/events", get(stream_events))
        .route("/run/events", post(record_run_event))
        .route("/run/records", get(replay_run_records))
        .route("/run/spans", post(record_run_spans))
        .route("/commands/cancel", post(cancel_command_submission))
        .route("/traces", get(list_traces))
        .route("/traces/{name}", get(download_trace))
        .route("/leases/release", post(release_lease))
        .route("/leases/camera", post(report_lease_camera))
        .route("/arms/{arm}/stop", post(stop_leased_arm))
        .route(
            "/action_sequence",
            post(super::program_http::execute_source),
        )
        .route("/compilations/run", post(super::program_http::run_program))
        .route(
            "/compilations/{id}",
            get(super::program_http::inspect_compilation),
        )
        .route("/action_sequence/{id}/cancel", post(cancel_action_sequence))
        .route("/state", get(read_state))
        .route("/action_sequence", get(read_action_sequence))
        .route("/action_sequence/{id}", get(read_action_sequence_by_id))
        .route("/commands/{id}", get(read_command))
        .with_state(station_state);
    match viewer_directory {
        Some(directory) => router
            .route("/", get(|| async { Redirect::temporary("/viewer/") }))
            .nest_service(
                "/viewer",
                // WHY: A rebuilt viewer keeps its file names, so revalidate each time.
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

async fn read_state(State(station_state): State<Arc<StationState>>) -> Response {
    let Some(arms) = station_state.arms.get() else {
        return arms_not_set_up();
    };
    match crate::station_api::state::station_state(arms).await {
        Some(state) => Json(state).into_response(),
        None => arms_not_set_up(),
    }
}

async fn read_action_sequence(State(station_state): State<Arc<StationState>>) -> Response {
    let current = station_state
        .executions
        .current()
        .map(|receiver| receiver.borrow().clone());
    Json(current).into_response()
}

async fn read_action_sequence_by_id(
    State(station_state): State<Arc<StationState>>,
    UrlPath(id): UrlPath<String>,
) -> Response {
    match station_state.executions.lookup(&id) {
        Some(action_sequence) => Json(action_sequence).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn read_command(
    State(station_state): State<Arc<StationState>>,
    UrlPath(command_id): UrlPath<String>,
) -> Response {
    let Some(arms) = station_state.arms.get() else {
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
    State(station_state): State<Arc<StationState>>,
    Json(request): Json<hebbian_contracts::PosePreviewBatchRequest>,
) -> Result<Json<hebbian_contracts::PosePreviewBatchResponse>, (StatusCode, String)> {
    execute_pose_previews(station_state, request)
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

async fn cancel_action_sequence(
    State(station_state): State<Arc<StationState>>,
    UrlPath(id): UrlPath<String>,
) -> Result<axum::http::StatusCode, (axum::http::StatusCode, String)> {
    if station_state.executions.cancel(&id) {
        Ok(axum::http::StatusCode::NO_CONTENT)
    } else {
        Err((
            axum::http::StatusCode::NOT_FOUND,
            "ActionSequence not found".to_owned(),
        ))
    }
}

async fn report_lease_camera(
    State(station_state): State<Arc<StationState>>,
    Json(input): Json<hebbian_contracts::LeaseCameraReport>,
) -> Result<axum::http::StatusCode, (axum::http::StatusCode, String)> {
    let link = station_state.lease_link()?;
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
    State(station_state): State<Arc<StationState>>,
    UrlPath(arm): UrlPath<String>,
    Json(input): Json<hebbian_contracts::LeaseToken>,
) -> Result<StatusCode, (StatusCode, String)> {
    let link = station_state
        .arms
        .get()
        .ok_or((
            StatusCode::SERVICE_UNAVAILABLE,
            "Station arms are not ready".to_owned(),
        ))?
        .get(&arm)
        .ok_or((StatusCode::NOT_FOUND, "Unknown arm".to_owned()))?;
    leased_stop_response(
        stop_leased_arms([link], &input.lease_token),
        StatusCode::FORBIDDEN,
    )
}

fn stop_leased_arms<'a>(
    links: impl IntoIterator<Item = &'a LinkHandle>,
    lease_token: &str,
) -> LeasedStopOutcome {
    let mut outcome = LeasedStopOutcome::Submitted;
    for link in links {
        match link.stop_if_leased(Some(lease_token)) {
            LeasedStopOutcome::Submitted => {}
            LeasedStopOutcome::LeaseLost => return LeasedStopOutcome::LeaseLost,
            LeasedStopOutcome::Unavailable => outcome = LeasedStopOutcome::Unavailable,
        }
    }
    outcome
}

fn leased_stop_response(
    outcome: LeasedStopOutcome,
    lease_lost_status: StatusCode,
) -> Result<StatusCode, (StatusCode, String)> {
    match outcome {
        LeasedStopOutcome::Submitted => Ok(StatusCode::NO_CONTENT),
        LeasedStopOutcome::LeaseLost => Err((lease_lost_status, "Lease lost".to_owned())),
        LeasedStopOutcome::Unavailable => Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "Stop could not be submitted".to_owned(),
        )),
    }
}

async fn release_lease(
    State(station_state): State<Arc<StationState>>,
    Json(input): Json<hebbian_contracts::LeaseToken>,
) -> Result<axum::http::StatusCode, (axum::http::StatusCode, String)> {
    let link = station_state.lease_link()?;
    if link.release_lease_with(&input.lease_token, || {
        station_state.executions.cancel_owner(&input.lease_token)
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
    State(station_state): State<Arc<StationState>>,
    Json(input): Json<hebbian_contracts::LeaseToken>,
) -> Result<axum::http::StatusCode, (axum::http::StatusCode, String)> {
    let link = station_state.lease_link()?;
    if link.retain_lease(&input.lease_token) {
        Ok(axum::http::StatusCode::NO_CONTENT)
    } else {
        Err((
            axum::http::StatusCode::CONFLICT,
            "The lease has expired or is no longer held".to_owned(),
        ))
    }
}

pub(super) fn validate_submission_id(
    station_state: &StationState,
    station_instance_id: &str,
    request_id: &str,
) -> Result<uuid::Uuid, (axum::http::StatusCode, String)> {
    if station_instance_id != station_state.instance_id {
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
    State(station_state): State<Arc<StationState>>,
    Json(input): Json<hebbian_contracts::CommandCancellationRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    use crate::station_api::submissions::CancellationResult;

    let id = validate_submission_id(
        &station_state,
        &input.station_instance_id,
        &input.request_id,
    )?;
    let arms = station_state.arms.get().ok_or((
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
    let Some(lease_link) = links.first() else {
        return Err((
            StatusCode::BAD_REQUEST,
            "Expected at least one arm".to_owned(),
        ));
    };
    if !lease_link.with_lease(&input.lease_token, &mut || {}) {
        return Err((StatusCode::CONFLICT, "Lease lost".to_owned()));
    }
    let response =
        match station_state
            .command_submissions
            .cancel(id, &input.lease_token, &input.arms)
        {
            CancellationResult::Fenced => return Ok(StatusCode::NO_CONTENT),
            CancellationResult::Expired => {
                return Err((
                    StatusCode::GONE,
                    "Request outcome is no longer retained".to_owned(),
                ));
            }
            CancellationResult::Pending(response) => response,
            CancellationResult::Conflict => {
                return Err((
                    StatusCode::CONFLICT,
                    "Cancellation does not match the submission".to_owned(),
                ));
            }
        };
    // INVARIANT: Once a cancellation is received, finish processing it even if
    // the caller closes the HTTP connection while admission is still pending.
    tokio::spawn(cancel_admitted_commands(
        station_state,
        input.lease_token,
        response,
    ))
    .await
    .map_err(|_| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "Cancellation outcome is unknown".to_owned(),
        )
    })?
}

async fn cancel_admitted_commands(
    station_state: Arc<StationState>,
    lease_token: String,
    response: tokio::sync::watch::Receiver<CommandSubmissionOutcome>,
) -> Result<StatusCode, (StatusCode, String)> {
    let arms = station_state.arms.get().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "Station arms are not ready".to_owned(),
    ))?;
    let outcome = wait_for_outcome(response)
        .await
        .map_err(|_| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "Command outcome is unknown".to_owned(),
            )
        })?
        .map_err(|detail| (StatusCode::SERVICE_UNAVAILABLE, detail))?;
    if let hebbian_contracts::CommandAnswer::Accepted { records } = outcome {
        for record in records
            .into_iter()
            .filter(|entry| entry.record.completion.is_none())
        {
            let Some(link) = arms.get(&record.arm) else {
                continue;
            };
            leased_stop_response(
                link.cancel_leased_command(&lease_token, record.record.command_id),
                StatusCode::CONFLICT,
            )?;
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

type CommandSubmissionOutcome = Option<Result<hebbian_contracts::CommandAnswer, String>>;

fn follow_or_start_submission(
    reservation: crate::station_api::submissions::SubmissionReservation,
    start: impl FnOnce(
        tokio::sync::watch::Sender<CommandSubmissionOutcome>,
        tokio::sync::watch::Receiver<bool>,
    ) -> tokio::sync::watch::Receiver<CommandSubmissionOutcome>,
) -> Result<tokio::sync::watch::Receiver<CommandSubmissionOutcome>, (StatusCode, String)> {
    use crate::station_api::submissions::SubmissionReservation;
    match reservation {
        SubmissionReservation::Conflict => Err((
            axum::http::StatusCode::CONFLICT,
            "Request ID already used for a different command".to_owned(),
        )),
        SubmissionReservation::Expired => Err((
            axum::http::StatusCode::GONE,
            "Request outcome is no longer retained".to_owned(),
        )),
        SubmissionReservation::Full => Err((
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "Too many command submissions are still in flight".to_owned(),
        )),
        SubmissionReservation::Existing(response) => Ok(response),
        SubmissionReservation::Cancelled => {
            let (_, response) =
                tokio::sync::watch::channel(Some(Ok(hebbian_contracts::CommandAnswer::Refused {
                    reason: hebbian_contracts::RejectionReason::ArmBusy,
                    detail: "Command submission was cancelled".to_owned(),
                })));
            Ok(response)
        }
        SubmissionReservation::New(sender, cancellation) => Ok(start(sender, cancellation)),
    }
}

type CommandAdmission<ReplyContext, ReplyRecords> = Result<
    (
        ReplyContext,
        oneshot::Receiver<Result<ReplyRecords, crate::arm::Rejection>>,
    ),
    (hebbian_contracts::RejectionReason, String),
>;

async fn command_submission_outcome<ReplyContext, ReplyRecords>(
    admitted: Result<CommandAdmission<ReplyContext, ReplyRecords>, tokio::task::JoinError>,
    into_arm_records: impl FnOnce(
        ReplyContext,
        ReplyRecords,
    ) -> Result<Vec<hebbian_contracts::ArmRecord>, String>,
) -> Result<hebbian_contracts::CommandAnswer, String> {
    let (context, reply) = match admitted {
        Err(_) => return Err("Command worker failed; outcome is unknown".to_owned()),
        Ok(Err((reason, detail))) => {
            return Ok(hebbian_contracts::CommandAnswer::Refused { reason, detail });
        }
        Ok(Ok(pending)) => pending,
    };
    match reply.await {
        Ok(Ok(records)) => into_arm_records(context, records)
            .map(|records| hebbian_contracts::CommandAnswer::Accepted { records }),
        Ok(Err(error)) => Ok(hebbian_contracts::CommandAnswer::Refused {
            reason: error.reason,
            detail: error.detail,
        }),
        Err(_) => Err("Control loop reply lost; command outcome is unknown".to_owned()),
    }
}

type SubmissionGeneration = (Arc<std::sync::atomic::AtomicU64>, u64);

async fn submit_station_command<ReplyContext: Send + 'static, ReplyRecords: Send + 'static>(
    station: Arc<StationState>,
    request: super::submissions::SubmissionRequest,
    admit: impl FnOnce(
        &StationState,
        &BTreeMap<String, LinkHandle>,
        Vec<SubmissionGeneration>,
        tokio::sync::watch::Receiver<bool>,
    ) -> CommandAdmission<ReplyContext, ReplyRecords>
    + Send
    + 'static,
    into_arm_records: impl FnOnce(
        ReplyContext,
        ReplyRecords,
    ) -> Result<Vec<hebbian_contracts::ArmRecord>, String>
    + Send
    + 'static,
) -> Result<Json<hebbian_contracts::CommandAnswer>, (StatusCode, String)> {
    use super::submissions::SubmissionRequest;
    let (station_instance_id, request_id) = match &request {
        SubmissionRequest::Single(input) => (&input.station_instance_id, &input.request_id),
        SubmissionRequest::Group(input) => (&input.station_instance_id, &input.request_id),
    };
    let id = validate_submission_id(&station, station_instance_id, request_id)?;
    let links = station
        .arms
        .get()
        .ok_or((
            StatusCode::SERVICE_UNAVAILABLE,
            "Station arms are not ready".to_owned(),
        ))?
        .clone();
    let targets = match &request {
        SubmissionRequest::Single(input) => vec![&input.arm],
        SubmissionRequest::Group(input) => input.command.targets.keys().collect(),
    };
    let generation = targets
        .into_iter()
        .filter_map(|arm| links.get(arm))
        .map(LinkHandle::submission_generation)
        .collect();
    let reservation = station.command_submissions.reserve(id, request);
    let response = follow_or_start_submission(reservation, |sender, cancellation| {
        SubmissionCompletion::start(
            sender,
            "Command worker failed; outcome is unknown".to_owned(),
            |completion| {
                tokio::spawn(async move {
                    let admitted = tokio::task::spawn_blocking(move || {
                        admit(&station, &links, generation, cancellation)
                    })
                    .await;
                    completion
                        .publish(command_submission_outcome(admitted, into_arm_records).await);
                });
            },
        )
    })?;
    wait_for_outcome(response)
        .await
        .map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Command outcome is unknown".to_owned(),
            )
        })?
        .map(Json)
        .map_err(|detail| (StatusCode::INTERNAL_SERVER_ERROR, detail))
}

// INVARIANT: Hold the watch read lock through enqueue, so cancel either
// prevents admission or waits for the exact command IDs from the controller.
fn fence_cancelled_submission(
    cancellation: &tokio::sync::watch::Receiver<bool>,
) -> Result<tokio::sync::watch::Ref<'_, bool>, (hebbian_contracts::RejectionReason, String)> {
    let cancelled = cancellation.borrow();
    if *cancelled {
        return Err((
            hebbian_contracts::RejectionReason::ArmBusy,
            "Command submission was cancelled".to_owned(),
        ));
    }
    Ok(cancelled)
}

async fn submit_group_command(
    State(station_state): State<Arc<StationState>>,
    Json(input): Json<hebbian_contracts::GroupCommandSubmissionRequest>,
) -> Result<Json<hebbian_contracts::CommandAnswer>, (axum::http::StatusCode, String)> {
    submit_station_command(
        station_state,
        super::submissions::SubmissionRequest::Group(input.clone()),
        move |station, links, generation, cancellation| {
            let token = input.lease_token.as_deref();
            let targets = input
                .command
                .targets
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>();
            station
                .executions
                .check_command_conflict(token, &targets)
                .map_err(|detail| {
                    (
                        hebbian_contracts::RejectionReason::ArmBusy,
                        detail.to_owned(),
                    )
                })?;
            let (link, moves, gates) = crate::station_api::commands::plan_group_move(
                links,
                &input.command,
                &input.expected_revisions,
            )?;
            let arms = moves
                .iter()
                .map(|part| part.arm.clone())
                .collect::<Vec<_>>();
            let _admission = fence_cancelled_submission(&cancellation)?;
            let reply = link
                .move_arms_async(moves, token, input.trace_context, &gates, &generation)
                .map_err(|error| (error.reason, error.detail))?;
            Ok::<_, (hebbian_contracts::RejectionReason, String)>((arms, reply))
        },
        |arms, records| {
            if arms.len() != records.len() {
                return Err("Control loop returned an unexpected number of records".to_owned());
            }
            Ok(arms
                .into_iter()
                .zip(records)
                .map(|(arm, record)| hebbian_contracts::ArmRecord { arm, record })
                .collect())
        },
    )
    .await
}

async fn submit_command(
    State(station_state): State<Arc<StationState>>,
    Json(input): Json<hebbian_contracts::CommandSubmissionRequest>,
) -> Result<Json<hebbian_contracts::CommandAnswer>, (axum::http::StatusCode, String)> {
    submit_station_command(
        station_state,
        super::submissions::SubmissionRequest::Single(input.clone()),
        move |station, links, mut generation, cancellation| {
            let token = input.lease_token.as_deref();
            let link = links.get(&input.arm).ok_or((
                hebbian_contracts::RejectionReason::InvalidTarget,
                "Unknown arm".to_owned(),
            ))?;
            if matches!(input.command, hebbian_contracts::StationCommand::Stop) {
                let _admission = fence_cancelled_submission(&cancellation)?;
                return link
                    .interrupt_async(input.trace_context)
                    .map(|reply| (input.arm, reply))
                    .map_err(|error| (error.reason, error.detail));
            }
            let disabling = matches!(input.command, hebbian_contracts::StationCommand::Disable);
            if !disabling
                && let Err(detail) = station
                    .executions
                    .check_command_conflict(token, &[&input.arm])
            {
                return Err((
                    hebbian_contracts::RejectionReason::ArmBusy,
                    detail.to_owned(),
                ));
            }
            let command =
                crate::station_api::commands::arm_command(link, &input.arm, input.command)?;
            let _admission = fence_cancelled_submission(&cancellation)?;
            if disabling && let Some(token) = token {
                link.stop_leased_motion_for_submission(token, &mut generation, || {
                    station.executions.cancel_owner(token)
                })
                .map_err(|error| (error.reason, error.detail))?;
            }
            link.submit_command_async(
                command,
                token,
                input.trace_context,
                input.expected_revision,
                &generation,
            )
            .map(|reply| (input.arm, reply))
            .map_err(|error| (error.reason, error.detail))
        },
        |arm, record| Ok(vec![hebbian_contracts::ArmRecord { arm, record }]),
    )
    .await
}

async fn record_run_event(
    State(station_state): State<Arc<StationState>>,
    Json(event): Json<hebbian_contracts::RunEvent>,
) -> axum::http::StatusCode {
    station_state.telemetry.record_event(event);
    axum::http::StatusCode::NO_CONTENT
}

#[derive(serde::Deserialize)]
struct RunReplayQuery {
    after: u64,
}

async fn replay_run_records(
    State(station_state): State<Arc<StationState>>,
    Query(query): Query<RunReplayQuery>,
) -> Json<Vec<hebbian_contracts::ServerMessage>> {
    let mut cursor = query.after;
    let updates = station_state.telemetry.updates(&mut cursor, &mut None);
    Json(updates.records)
}

async fn record_run_spans(
    State(station_state): State<Arc<StationState>>,
    Json(request): Json<hebbian_contracts::RunSpansRequest>,
) -> axum::http::StatusCode {
    station_state.telemetry.record_spans(request.spans);
    axum::http::StatusCode::NO_CONTENT
}

fn parse_event_cursor(
    headers: &axum::http::HeaderMap,
) -> Result<Option<(String, u64)>, (StatusCode, String)> {
    headers
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
        })
}

fn encode_station_event(
    instance_id: &str,
    event: &super::events::StationEvent,
) -> Result<axum::response::sse::Event, axum::Error> {
    axum::response::sse::Event::default()
        .event("station")
        .id(format!("{instance_id}:{}", event.sequence))
        .json_data(event.payload.as_ref())
}

async fn stream_events(
    State(station_state): State<Arc<StationState>>,
    headers: axum::http::HeaderMap,
) -> Result<impl axum::response::IntoResponse, (axum::http::StatusCode, String)> {
    use crate::station_api::events::EventSubscription;
    use axum::response::sse::Event;
    #[derive(Serialize)]
    struct Snapshot<'a> {
        station_instance_id: &'a str,
        sequence: u64,
        run_sequence: u64,
        items: Vec<&'a super::events::StreamPayload>,
    }
    let cursor = parse_event_cursor(&headers)?;
    let (initial, mut updates) = match cursor {
        None => {
            let (sequence, snapshot, updates) = station_state.events.subscribe_with_snapshot();
            if !snapshot.iter().any(|event| event.is_station_state()) {
                return Err((
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Station snapshot is not ready".to_owned(),
                ));
            }
            let event = Event::default()
                .event("snapshot")
                .id(format!("{}:{sequence}", station_state.instance_id))
                .json_data(Snapshot {
                    station_instance_id: &station_state.instance_id,
                    sequence,
                    run_sequence: station_state.telemetry.newest_seq(),
                    items: snapshot.iter().map(AsRef::as_ref).collect(),
                })
                .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
            (vec![event], updates)
        }
        Some((instance, sequence)) if instance == station_state.instance_id => {
            match station_state.events.subscribe_after(sequence) {
                EventSubscription::Replay { events, updates } => (
                    events
                        .iter()
                        .map(|event| encode_station_event(&station_state.instance_id, event))
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?,
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
    let (sender, receiver) = tokio::sync::mpsc::channel::<Result<Event, axum::Error>>(64);
    let instance_id = station_state.instance_id.clone();
    tokio::spawn(async move {
        for event in initial {
            if sender.send(Ok(event)).await.is_err() {
                return;
            }
        }
        loop {
            let update = tokio::select! {
                () = sender.closed() => return,
                update = updates.recv() => update,
            };
            match update {
                Ok(event) => {
                    if sender
                        .send(encode_station_event(&instance_id, &event))
                        .await
                        .is_err()
                    {
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
    State(station_state): State<Arc<StationState>>,
) -> Json<hebbian_contracts::StationInstance> {
    Json(hebbian_contracts::StationInstance {
        id: station_state.instance_id.clone(),
    })
}

async fn acquire_lease(
    State(station_state): State<Arc<StationState>>,
    Json(input): Json<hebbian_contracts::LeaseAcquireRequest>,
) -> Result<Json<hebbian_contracts::LeaseGrant>, (StatusCode, String)> {
    use crate::station_api::submissions::LeaseReservation;

    let id = validate_submission_id(
        &station_state,
        &input.station_instance_id,
        &input.request_id,
    )?;
    let links = station_state
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
    let response = match station_state.lease_submissions.reserve(id, input.clone()) {
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
        LeaseReservation::New(sender) => SubmissionCompletion::start(
            sender,
            "Lease worker failed; outcome is unknown".to_owned(),
            |completion| {
                tokio::task::spawn_blocking(move || {
                    let result = (|| {
                        if input.request.holder == hebbian_contracts::LeaseHolder::Manual {
                            crate::lease::manual_control_ready_without_waiting(&links)?;
                        }
                        link.acquire_lease(input.request, input.take)
                            .map_err(|error| error.to_string())
                    })();
                    completion.publish(result);
                });
            },
        ),
    };
    wait_for_outcome(response)
        .await
        .map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Lease outcome is unknown".to_owned(),
            )
        })?
        .map(Json)
        .map_err(|detail| (StatusCode::CONFLICT, detail))
}

pub(crate) async fn execute_pose_previews(
    station_state: Arc<StationState>,
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
    let arms = station_state
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

pub const VIEWER_DIRECTORY_VARIABLE: &str = "HEBBIAN_VIEWER_DIRECTORY";

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

async fn list_traces(State(station_state): State<Arc<StationState>>) -> Response {
    let directories = station_state.trace_directories.clone();
    match tokio::task::spawn_blocking(move || trace_files(&directories)).await {
        Ok(files) => {
            Json(files.into_iter().map(|(file, _)| file).collect::<Vec<_>>()).into_response()
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

// SAFETY: Resolve trace names from the directory listing, never by joining untrusted names to a path.
fn find_trace(directories: &[PathBuf], name: &str) -> Option<PathBuf> {
    trace_files(directories)
        .into_iter()
        .find(|(file, _)| file.name == name)
        .map(|(_, path)| path)
}

async fn download_trace(
    State(station_state): State<Arc<StationState>>,
    UrlPath(name): UrlPath<String>,
) -> Response {
    let directories = station_state.trace_directories.clone();
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
    use super::{HttpServer, ServerMessage, StationState, stream_events};
    use axum::extract::State;
    use axum::http::{HeaderMap, HeaderValue, StatusCode};
    use std::sync::{Arc, OnceLock};

    fn ready_state() -> ServerMessage {
        ServerMessage::State {
            state: Box::new(hebbian_contracts::StationState {
                robot: None,
                description: None,
                joint_names: vec![],
                surface_guard: None,
                jaw_closing_axis: hebbian_contracts::JawClosingAxis::X,
                max_joint_speed: 1.0,
                arms: vec![],
                lease: None,
            }),
        }
    }

    fn station() -> Arc<StationState> {
        Arc::new(StationState {
            shutdown_requested: Arc::default(),
            program_submissions: crate::station_api::submissions::ProgramSubmissions::default(),
            compiler: crate::station_api::programs::ProgramStore::default(),
            compilation_targets: OnceLock::new(),
            trace_directories: vec![],
            arms: OnceLock::new(),
            telemetry: Arc::new(crate::station_api::telemetry::DiscardTelemetry),
            executions: crate::station_api::runtime::ExecutionRegistry::default(),
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
            Arc::default(),
            |_, _| Ok(axum::Router::new()),
        )
        .unwrap();
        server.station_state.events.publish(ready_state());
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
                .station_state
                .events
                .resync_arm("replayed".to_owned());
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
        let station_state = station();
        let error = stream_events(State(Arc::clone(&station_state)), HeaderMap::new())
            .await
            .err()
            .unwrap();
        assert_eq!(error.0, StatusCode::SERVICE_UNAVAILABLE);
        station_state.events.publish(ready_state());
        assert!(
            stream_events(State(Arc::clone(&station_state)), HeaderMap::new())
                .await
                .is_ok()
        );
        station_state.events.invalidate_station_state();
        let error = stream_events(State(Arc::clone(&station_state)), HeaderMap::new())
            .await
            .err()
            .unwrap();
        assert_eq!(error.0, StatusCode::SERVICE_UNAVAILABLE);
        let mut headers = HeaderMap::new();
        headers.insert("last-event-id", HeaderValue::from_static("old-instance:1"));
        let error = stream_events(State(station_state), headers)
            .await
            .err()
            .unwrap();
        assert_eq!(error.0, StatusCode::CONFLICT);
    }
}

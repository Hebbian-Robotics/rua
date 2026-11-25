use hebbian_contracts::{LeasedStopOutcome, ServerMessage, TraceFile};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, UNIX_EPOCH};

use anyhow::Context;
use async_compression::tokio::bufread::GzipEncoder;
use axum::Json;
use axum::Router;
use axum::body::Body;
use axum::extract::{Path as UrlPath, Query, Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde::Serialize;
use tokio::io::BufReader;
use tokio::sync::oneshot;
use tokio_util::io::ReaderStream;

use crate::control_link::{LinkHandle, SubmissionAuthority};

use super::admission::{AdmissionClosed, AdmissionState};
use super::http_state::{SHUTTING_DOWN, SnapshotSubscription, StationHttpState};
use super::submissions::{
    Admission, CommandOutcomeUnknown, OperationId, PendingAdmission, SubmissionCompletion,
    TerminalMotion, wait_for_outcome,
};

const WORKER_THREADS: usize = 2;
const PREVIEW_WORKERS: usize = 4;
const OPERATION_WAIT: Duration = Duration::from_secs(20);
const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);

#[derive(Serialize)]
struct Health {
    status: &'static str,
}

pub struct HttpServer {
    station_state: Arc<StationHttpState>,
    runtime: Option<tokio::runtime::Runtime>,
    shutdown: Option<oneshot::Sender<()>>,
    address: SocketAddr,
}

impl HttpServer {
    // CONSTRAINT: Extra HTTP routes run outside the motor loop and must never make the control path wait on network I/O.
    pub fn start(
        address: &str,
        trace_directories: Vec<PathBuf>,
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
        let station_state = Arc::new(StationHttpState::new(
            runtime.handle(),
            shutdown_requested,
            trace_directories,
            telemetry,
        ));
        let extra = extra_routes(runtime.handle(), bound)?;
        let router = router(Arc::clone(&station_state)).merge(extra);
        let (shutdown, shutdown_requested) = oneshot::channel::<()>();
        runtime.spawn(async move {
            let http_server = axum::serve(
                listener,
                router.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .with_graceful_shutdown(async {
                let _ = shutdown_requested.await;
            });
            if let Err(error) = http_server.await {
                tracing::error!(%error, "The HTTP server stopped");
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

    pub fn attach_sequence_cameras(&self, source: super::SequenceCameraSource) {
        let _ = self.station_state.sequence_cameras.set(source);
    }

    pub fn attach_arms(&self, arms: BTreeMap<String, LinkHandle>) {
        self.station_state.attach_arms(
            arms,
            self.runtime.as_ref().map(tokio::runtime::Runtime::handle),
        );
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
    State(station_state): State<Arc<StationHttpState>>,
    request: Request,
    next: Next,
) -> Response {
    if station_state.is_shutting_down() {
        return (StatusCode::SERVICE_UNAVAILABLE, SHUTTING_DOWN).into_response();
    }
    next.run(request).await
}

fn router(station_state: Arc<StationHttpState>) -> Router {
    let admission_routes = Router::new()
        .route("/commands", post(submit_command))
        .route("/group_commands", post(submit_group_command))
        .route("/operations/{id}/goal", post(update_stream_goal))
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
    Router::new()
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
        .route("/operations/{id}/cancel", post(cancel_operation))
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
        .route(
            "/action_sequence/{id}/completion",
            get(wait_action_sequence_completion),
        )
        .route(
            "/action_sequence/{id}/images",
            get(super::sequence_images::read_images),
        )
        .route("/operations/{id}", get(read_operation))
        .route("/operations/{id}/completion", post(wait_operation))
        .with_state(station_state)
}

fn arms_not_set_up() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        "The arms are not set up yet",
    )
        .into_response()
}

async fn read_state(State(station_state): State<Arc<StationHttpState>>) -> Response {
    let Some(arms) = station_state.arms() else {
        return arms_not_set_up();
    };
    match crate::station_api::state::station_state(arms).await {
        Some(state) => Json(state).into_response(),
        None => arms_not_set_up(),
    }
}

async fn read_action_sequence(State(station_state): State<Arc<StationHttpState>>) -> Response {
    match station_state
        .admission
        .decide(|state| {
            state
                .executions
                .current()
                .map(|receiver| receiver.borrow().clone())
        })
        .await
    {
        Ok(current) => Json(current).into_response(),
        Err(error) => admission_unavailable(error).into_response(),
    }
}

async fn read_action_sequence_by_id(
    State(station_state): State<Arc<StationHttpState>>,
    UrlPath(id): UrlPath<String>,
) -> Response {
    match station_state
        .admission
        .decide(move |state| state.executions.lookup(&id))
        .await
    {
        Ok(Some(action_sequence)) => Json(action_sequence).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(error) => admission_unavailable(error).into_response(),
    }
}

fn parse_operation_id(value: &str) -> Result<OperationId, (StatusCode, String)> {
    OperationId::parse(value).ok_or((
        StatusCode::BAD_REQUEST,
        "Expected a UUID v4 operation ID".to_owned(),
    ))
}

async fn wait_action_sequence_completion(
    State(station_state): State<Arc<StationHttpState>>,
    UrlPath(id): UrlPath<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let mut progress = station_state
        .admission
        .decide(move |state| state.executions.progress_for(&id))
        .await
        .map_err(admission_unavailable)?
        .ok_or((
            StatusCode::NOT_FOUND,
            "Action sequence is not retained".to_owned(),
        ))?;
    let outcome = tokio::time::timeout(
        OPERATION_WAIT,
        progress.wait_for(|record| {
            !matches!(
                record.phase,
                hebbian_contracts::ActionSequencePhase::Running { .. }
            )
        }),
    )
    .await;
    let record = match outcome {
        Ok(Ok(record)) => Some(record.clone()),
        Ok(Err(_)) => {
            return Err((
                StatusCode::SERVICE_UNAVAILABLE,
                "Action sequence progress is unavailable".to_owned(),
            ));
        }
        Err(_) => None,
    };
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(record)))
}

async fn read_operation(
    State(station_state): State<Arc<StationHttpState>>,
    UrlPath(id): UrlPath<String>,
) -> Result<Json<hebbian_contracts::OperationStatus>, (StatusCode, String)> {
    let operation_id = parse_operation_id(&id)?;
    station_state
        .admission
        .decide(move |state| state.operations.status(operation_id))
        .await
        .map_err(admission_unavailable)?
        .map(Json)
        .ok_or((
            StatusCode::NOT_FOUND,
            format!("Operation {id} is not retained"),
        ))
}

async fn wait_operation(
    State(station_state): State<Arc<StationHttpState>>,
    UrlPath(id): UrlPath<String>,
    Json(instance): Json<hebbian_contracts::StationInstance>,
) -> Result<Json<Option<hebbian_contracts::OperationCompletion>>, (StatusCode, String)> {
    station_state.require_station_instance(&instance.id)?;
    let operation_id = parse_operation_id(&id)?;
    let operations = station_state
        .admission
        .decide(|state| state.operations.clone())
        .await
        .map_err(admission_unavailable)?;
    let completion = tokio::time::timeout(OPERATION_WAIT, async {
        let outcome = operations.wait_for_completion(operation_id).await.ok_or((
            StatusCode::NOT_FOUND,
            format!("Operation {id} is not retained"),
        ))?;
        let records = match outcome {
            TerminalMotion::Finished(records) => records,
            TerminalMotion::Refused { detail, .. } => return Err((StatusCode::CONFLICT, detail)),
            TerminalMotion::CancelledBeforeAdmission => {
                return Err((
                    StatusCode::CONFLICT,
                    "Operation was cancelled before admission".to_owned(),
                ));
            }
            TerminalMotion::OutcomeUnknown(detail) => {
                return Err((StatusCode::INTERNAL_SERVER_ERROR, detail.to_string()));
            }
        };
        let sequence = station_state.state_reflecting(&records).await.ok_or((
            StatusCode::SERVICE_UNAVAILABLE,
            "Station state is unavailable".to_owned(),
        ))?;
        Ok(hebbian_contracts::OperationCompletion {
            records,
            state_sequence: sequence.to_string(),
        })
    })
    .await;
    match completion {
        Ok(result) => result.map(|completion| Json(Some(completion))),
        Err(_) => Ok(Json(None)),
    }
}

async fn preview_poses(
    State(station_state): State<Arc<StationHttpState>>,
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
    State(station_state): State<Arc<StationHttpState>>,
    UrlPath(id): UrlPath<String>,
) -> Result<axum::http::StatusCode, (axum::http::StatusCode, String)> {
    if station_state
        .admission
        .decide(move |state| state.executions.cancel(&id))
        .await
        .map_err(admission_unavailable)?
    {
        Ok(axum::http::StatusCode::NO_CONTENT)
    } else {
        Err((
            axum::http::StatusCode::NOT_FOUND,
            "ActionSequence not found".to_owned(),
        ))
    }
}

async fn report_lease_camera(
    State(station_state): State<Arc<StationHttpState>>,
    Json(input): Json<hebbian_contracts::LeaseCameraReport>,
) -> Result<axum::http::StatusCode, (axum::http::StatusCode, String)> {
    use crate::lease::CameraReportRefusal;

    let link = station_state.lease_link()?.clone();
    let camera = input.camera.clone();
    match station_state
        .admission
        .decide(move |_| {
            link.report_camera(&input.lease_token, input.camera.as_str(), input.available)
        })
        .await
        .map_err(admission_unavailable)?
    {
        Ok(()) => Ok(StatusCode::NO_CONTENT),
        Err(CameraReportRefusal::LeaseLost) => Err((
            StatusCode::FORBIDDEN,
            "This token does not hold the lease".to_owned(),
        )),
        Err(CameraReportRefusal::CameraNotRequired) => Err((
            StatusCode::CONFLICT,
            format!("The lease does not require camera {camera}"),
        )),
    }
}

async fn stop_leased_arm(
    State(station_state): State<Arc<StationHttpState>>,
    UrlPath(arm): UrlPath<String>,
    Json(input): Json<hebbian_contracts::LeaseToken>,
) -> Result<StatusCode, (StatusCode, String)> {
    let link = station_state
        .arms()
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
    State(station_state): State<Arc<StationHttpState>>,
    Json(input): Json<hebbian_contracts::LeaseToken>,
) -> Result<axum::http::StatusCode, (axum::http::StatusCode, String)> {
    let link = station_state.lease_link()?.clone();
    if station_state
        .admission
        .decide(move |state| {
            link.release_lease_with(&input.lease_token, || {
                state.executions.cancel_owner(&input.lease_token)
            })
        })
        .await
        .map_err(admission_unavailable)?
    {
        Ok(axum::http::StatusCode::NO_CONTENT)
    } else {
        Err((
            axum::http::StatusCode::CONFLICT,
            "The lease has expired or is no longer held".to_owned(),
        ))
    }
}

async fn renew_lease(
    State(station_state): State<Arc<StationHttpState>>,
    Json(input): Json<hebbian_contracts::LeaseToken>,
) -> Result<axum::http::StatusCode, (axum::http::StatusCode, String)> {
    let link = station_state.lease_link()?.clone();
    station_state
        .admission
        .decide(move |_| {
            if link.retain_lease(&input.lease_token) {
                Ok(axum::http::StatusCode::NO_CONTENT)
            } else {
                Err((
                    axum::http::StatusCode::CONFLICT,
                    "The lease has expired or is no longer held".to_owned(),
                ))
            }
        })
        .await
        .map_err(admission_unavailable)?
}

async fn cancel_operation(
    State(station_state): State<Arc<StationHttpState>>,
    UrlPath(id): UrlPath<String>,
    Json(input): Json<hebbian_contracts::CommandCancellationRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    use super::submissions::{CancellationResult, CancellationTarget};

    let id = parse_operation_id(&id)?;
    station_state.require_station_instance(&input.station_instance_id)?;
    let lease_link = station_state.lease_link()?.clone();
    // INVARIANT: A cancellation handed to the station survives its HTTP caller.
    tokio::spawn(async move {
        let token = input.lease_token.clone();
        let cancellation = match station_state
            .admission
            .decide(move |state| {
                if lease_link.with_lease(&token, || {}).is_some() {
                    Ok(state.operations.cancel(id, &token))
                } else {
                    Err((StatusCode::CONFLICT, "Lease lost".to_owned()))
                }
            })
            .await
            .map_err(admission_unavailable)??
        {
            CancellationResult::AlreadyStopped => return Ok(StatusCode::NO_CONTENT),
            CancellationResult::Expired => {
                return Err((
                    StatusCode::GONE,
                    "Operation outcome is no longer retained".to_owned(),
                ));
            }
            CancellationResult::Conflict => {
                return Err((
                    StatusCode::CONFLICT,
                    "Operation belongs to another lease".to_owned(),
                ));
            }
            CancellationResult::Requested(target) => target,
        };

        let records = match cancellation {
            CancellationTarget::Commands(records) => records,
            CancellationTarget::AwaitAdmission(response) => {
                match wait_for_outcome(response).await {
                    Ok(Ok(hebbian_contracts::CommandAnswer::Accepted { records })) => records,
                    Ok(Ok(hebbian_contracts::CommandAnswer::Refused { .. })) => {
                        return Ok(StatusCode::NO_CONTENT);
                    }
                    Ok(Err(failure)) => {
                        return Err((StatusCode::SERVICE_UNAVAILABLE, failure.to_string()));
                    }
                    Err(_) => {
                        return Err((
                            StatusCode::SERVICE_UNAVAILABLE,
                            "Command outcome is unknown".to_owned(),
                        ));
                    }
                }
            }
        };
        let arms = station_state.arms().cloned().ok_or((
            StatusCode::SERVICE_UNAVAILABLE,
            "Station arms are not ready".to_owned(),
        ))?;
        station_state
            .admission
            .decide(move |_| {
                let mut failure = None;
                for entry in records
                    .into_iter()
                    .filter(|entry| entry.record.is_pending())
                {
                    if let Some(link) = arms.get(&entry.arm)
                        && let Err(error) = leased_stop_response(
                            link.cancel_leased_command(&input.lease_token, entry.record.command_id),
                            StatusCode::CONFLICT,
                        )
                    {
                        failure.get_or_insert(error);
                    }
                }
                failure.map_or(Ok(StatusCode::NO_CONTENT), Err)
            })
            .await
            .map_err(admission_unavailable)?
    })
    .await
    .map_err(|_| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "Cancellation outcome is unknown".to_owned(),
        )
    })?
}

type CommandSubmissionOutcome = Option<Admission>;

fn follow_or_start_submission(
    reservation: crate::station_api::submissions::SubmissionReservation,
    start: impl FnOnce(PendingAdmission, tokio::sync::watch::Receiver<bool>),
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
        SubmissionReservation::New {
            pending,
            response,
            cancellation,
        } => {
            start(pending, cancellation);
            Ok(response)
        }
    }
}

type CommandAdmission<ReplyContext, ReplyRecords> = Result<
    (
        ReplyContext,
        oneshot::Receiver<Result<ReplyRecords, crate::arm::Rejection>>,
    ),
    Rejection,
>;

async fn command_submission_outcome<ReplyContext, ReplyRecords>(
    admitted: Result<CommandAdmission<ReplyContext, ReplyRecords>, AdmissionClosed>,
    into_arm_records: impl FnOnce(
        ReplyContext,
        ReplyRecords,
    )
        -> Result<Vec<hebbian_contracts::ArmRecord>, CommandOutcomeUnknown>,
) -> Admission {
    let (context, reply) = match admitted {
        Err(_) => return Err(CommandOutcomeUnknown::AdmissionWorker),
        Ok(Err(Rejection { reason, detail })) => {
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
        Err(_) => Err(CommandOutcomeUnknown::ControllerReplyLost),
    }
}

use crate::arm::{Rejection, rejection};

fn admission_unavailable(_: AdmissionClosed) -> (StatusCode, String) {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        "Station admission is unavailable".to_owned(),
    )
}

async fn submit_station_command<
    ReplyContext: Send + 'static,
    ReplyRecords: Send + 'static,
    Commit: FnOnce(&mut AdmissionState) -> CommandAdmission<ReplyContext, ReplyRecords> + Send + 'static,
>(
    station: Arc<StationHttpState>,
    request: super::submissions::SubmissionRequest,
    prepare: impl FnOnce(
        &BTreeMap<String, LinkHandle>,
        Vec<crate::lease::GenerationTicket>,
        tokio::sync::watch::Receiver<bool>,
    ) -> Result<Commit, Rejection>
    + Send
    + 'static,
    into_arm_records: impl FnOnce(
        ReplyContext,
        ReplyRecords,
    )
        -> Result<Vec<hebbian_contracts::ArmRecord>, CommandOutcomeUnknown>
    + Send
    + 'static,
) -> Result<Json<hebbian_contracts::CommandAnswer>, (StatusCode, String)> {
    use super::submissions::SubmissionRequest;
    let (station_instance_id, operation_id) = match &request {
        SubmissionRequest::Single(input) => (&input.station_instance_id, &input.operation_id),
        SubmissionRequest::Group(input) => (&input.station_instance_id, &input.operation_id),
    };
    station.require_station_instance(station_instance_id)?;
    let id = parse_operation_id(operation_id)?;
    let links = station
        .arms()
        .ok_or((
            StatusCode::SERVICE_UNAVAILABLE,
            "Station arms are not ready".to_owned(),
        ))?
        .clone();
    let targets = match &request {
        SubmissionRequest::Single(input) => vec![&input.arm],
        SubmissionRequest::Group(input) => input.command.goals.keys().collect(),
    };
    let generation = targets
        .into_iter()
        .filter_map(|arm| links.get(arm))
        .map(LinkHandle::submission_generation)
        .collect();
    let response = station
        .admission
        .clone()
        .decide(move |state| {
            let reservation = state.operations.reserve(id, request);
            follow_or_start_submission(reservation, |pending, cancellation| {
                tokio::spawn(async move {
                    let prepared = tokio::task::spawn_blocking(move || {
                        prepare(&links, generation, cancellation)
                    })
                    .await;
                    let admitted = match prepared {
                        Ok(Ok(commit)) => station.admission.decide(commit).await,
                        Ok(Err(refusal)) => Ok(Err(refusal)),
                        Err(_) => Err(AdmissionClosed),
                    };
                    let outcome = command_submission_outcome(admitted, into_arm_records).await;
                    let Ok(unfinished) = station
                        .admission
                        .decide(move |_| pending.complete(outcome))
                        .await
                    else {
                        return;
                    };
                    if !unfinished.is_empty()
                        && let Some(links) = station.arms()
                    {
                        let result = await_motion_completion(links, unfinished).await;
                        let _ = station
                            .admission
                            .decide(move |state| state.operations.motion_finished(id, result))
                            .await;
                    }
                });
            })
        })
        .await
        .map_err(admission_unavailable)??;
    wait_for_outcome(response)
        .await
        .map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Command outcome is unknown".to_owned(),
            )
        })?
        .map(Json)
        .map_err(|failure| (StatusCode::INTERNAL_SERVER_ERROR, failure.to_string()))
}

async fn await_motion_completion(
    links: &BTreeMap<String, LinkHandle>,
    mut records: Vec<hebbian_contracts::ArmRecord>,
) -> Result<Vec<hebbian_contracts::ArmRecord>, CommandOutcomeUnknown> {
    // WHY: Each wait registers with the link that owns terminal controller records.
    // Register every member concurrently so a short move's outcome survives a
    // longer move on another arm.
    let mut completions = tokio::task::JoinSet::new();
    for (index, entry) in records
        .iter()
        .enumerate()
        .filter(|(_, entry)| entry.record.is_pending())
    {
        let link = links
            .get(&entry.arm)
            .cloned()
            .ok_or_else(|| CommandOutcomeUnknown::MissingArm(entry.arm.clone()))?;
        let arm = entry.arm.clone();
        let command_id = entry.record.command_id.clone();
        completions.spawn(async move {
            let record = link
                .await_completion(command_id)
                .await
                .ok_or(CommandOutcomeUnknown::MissingCompletion(arm))?;
            Ok::<_, CommandOutcomeUnknown>((index, record))
        });
    }
    while let Some(completion) = completions.join_next().await {
        let (index, record) = completion
            .map_err(|error| CommandOutcomeUnknown::CompletionWorker(error.to_string()))??;
        records[index].record = record;
    }
    Ok(records)
}

// INVARIANT: Hold the watch read lock through enqueue, so cancel either
// prevents admission or waits for the exact command IDs from the controller.
fn fence_cancelled_submission(
    cancellation: &tokio::sync::watch::Receiver<bool>,
) -> Result<tokio::sync::watch::Ref<'_, bool>, Rejection> {
    let cancelled = cancellation.borrow();
    if *cancelled {
        return Err(rejection(
            hebbian_contracts::RejectionReason::ArmBusy,
            "Command submission was cancelled".to_owned(),
        ));
    }
    Ok(cancelled)
}

fn arm_busy(detail: &str) -> Rejection {
    rejection(
        hebbian_contracts::RejectionReason::ArmBusy,
        detail.to_owned(),
    )
}

async fn submit_group_command(
    State(station_state): State<Arc<StationHttpState>>,
    Json(input): Json<hebbian_contracts::GroupCommandSubmissionRequest>,
) -> Result<Json<hebbian_contracts::CommandAnswer>, (axum::http::StatusCode, String)> {
    submit_station_command(
        station_state,
        super::submissions::SubmissionRequest::Group(input.clone()),
        move |links, generation, cancellation| {
            let (link, actions, gates) =
                super::commands::plan_group_move(links, &input.command, &input.expected_revisions)?;
            let link = link.clone();
            let arms: Vec<_> = actions
                .iter()
                .map(|action| action.arm().to_owned())
                .collect();
            Ok(move |state: &mut AdmissionState| {
                let token = input.lease_token.as_deref();
                let targets: Vec<_> = arms.iter().map(String::as_str).collect();
                state
                    .executions
                    .check_command_conflict(token, &targets)
                    .map_err(arm_busy)?;
                let reply = link.action_step_async(
                    actions,
                    SubmissionAuthority {
                        claim: token.into(),
                        generation: &generation,
                        clock: state.clock,
                    },
                    &gates,
                    input.trace_context,
                    &cancellation,
                )?;
                Ok((arms, reply))
            })
        },
        |arms, records| {
            if arms.len() != records.len()
                || arms
                    .iter()
                    .zip(&records)
                    .any(|(arm, record)| *arm != record.arm)
            {
                return Err(CommandOutcomeUnknown::UnexpectedArmRecords);
            }
            Ok(records)
        },
    )
    .await
}

async fn submit_command(
    State(station_state): State<Arc<StationHttpState>>,
    Json(input): Json<hebbian_contracts::CommandSubmissionRequest>,
) -> Result<Json<hebbian_contracts::CommandAnswer>, (axum::http::StatusCode, String)> {
    submit_station_command(
        station_state,
        super::submissions::SubmissionRequest::Single(input.clone()),
        move |links, mut generation, cancellation| {
            let link = links
                .get(&input.arm)
                .ok_or(rejection(
                    hebbian_contracts::RejectionReason::InvalidTarget,
                    "Unknown arm".to_owned(),
                ))?
                .clone();
            let command = super::commands::arm_command(&link, &input.arm, input.command)?;
            Ok(move |state: &mut AdmissionState| {
                let _admission = fence_cancelled_submission(&cancellation)?;
                let token = input.lease_token.as_deref();
                let reply = match command {
                    hebbian_contracts::Command::Stop => link.interrupt_async(input.trace_context),
                    hebbian_contracts::Command::Disable => {
                        if let Some(token) = token {
                            link.stop_leased_motion_for_submission(token, &mut generation, || {
                                state.executions.cancel_owner(token)
                            })?;
                        }
                        link.submit_command_async(
                            command,
                            SubmissionAuthority {
                                claim: token.into(),
                                generation: &generation,
                                clock: state.clock,
                            },
                            input.trace_context,
                            input.expected_revision,
                        )
                    }
                    command @ (hebbian_contracts::Command::Enable
                    | hebbian_contracts::Command::ApproveRecovery
                    | hebbian_contracts::Command::ClearFault
                    | hebbian_contracts::Command::Shutdown
                    | hebbian_contracts::Command::Move { .. }
                    | hebbian_contracts::Command::StreamJoints { .. }
                    | hebbian_contracts::Command::StreamPose { .. }
                    | hebbian_contracts::Command::ReachPose { .. }
                    | hebbian_contracts::Command::SetGripper { .. }) => {
                        state
                            .executions
                            .check_command_conflict(token, &[&input.arm])
                            .map_err(arm_busy)?;
                        link.submit_command_async(
                            command,
                            SubmissionAuthority {
                                claim: token.into(),
                                generation: &generation,
                                clock: state.clock,
                            },
                            input.trace_context,
                            input.expected_revision,
                        )
                    }
                }?;
                Ok((input.arm, reply))
            })
        },
        |arm, record| Ok(vec![hebbian_contracts::ArmRecord { arm, record }]),
    )
    .await
}

async fn update_stream_goal(
    State(station): State<Arc<StationHttpState>>,
    UrlPath(id): UrlPath<String>,
    Json(input): Json<hebbian_contracts::StreamGoalUpdateRequest>,
) -> Result<Json<hebbian_contracts::CommandAnswer>, (StatusCode, String)> {
    use hebbian_contracts::{ArmRecord, RejectionReason};
    station.require_station_instance(&input.station_instance_id)?;
    let id = parse_operation_id(&id)?;
    let links = station.arms().cloned().unwrap_or_default();
    let admitted = station
        .admission
        .decide(move |state| {
            state.operations.update_stream(
                id,
                &input.lease_token,
                input.sequence,
                |arm, controller_stream| {
                    state
                        .executions
                        .check_command_conflict(Some(&input.lease_token), &[arm])
                        .map_err(arm_busy)?;
                    let link = links.get(arm).ok_or(rejection(
                        RejectionReason::InvalidTarget,
                        "Unknown arm".to_owned(),
                    ))?;
                    let generation = [link.submission_generation()];
                    let command = super::commands::joint_stream(
                        link,
                        arm,
                        input.joints,
                        input.gripper_opening,
                        super::commands::StreamAdmission::Update(controller_stream),
                    )?;
                    let reply = link.submit_command_async(
                        command,
                        SubmissionAuthority {
                            claim: crate::lease::MotionClaim::Token(&input.lease_token),
                            generation: &generation,
                            clock: state.clock,
                        },
                        None,
                        input.expected_revision,
                    )?;
                    Ok((arm.to_owned(), reply))
                },
            )
        })
        .await;
    let outcome =
        command_submission_outcome(admitted, |arm, record| Ok(vec![ArmRecord { arm, record }]))
            .await;
    outcome
        .map(Json)
        .map_err(|failure| (StatusCode::SERVICE_UNAVAILABLE, failure.to_string()))
}

async fn record_run_event(
    State(station_state): State<Arc<StationHttpState>>,
    Json(event): Json<hebbian_contracts::RunEvent>,
) -> axum::http::StatusCode {
    station_state.record_run_event(event);
    axum::http::StatusCode::NO_CONTENT
}

#[derive(serde::Deserialize)]
struct RunReplayQuery {
    after: u64,
}

async fn replay_run_records(
    State(station_state): State<Arc<StationHttpState>>,
    Query(query): Query<RunReplayQuery>,
) -> Json<Vec<ServerMessage>> {
    Json(station_state.replay_run_records(query.after))
}

async fn record_run_spans(
    State(station_state): State<Arc<StationHttpState>>,
    Json(request): Json<hebbian_contracts::RunSpansRequest>,
) -> axum::http::StatusCode {
    station_state.record_run_spans(request.spans);
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
    State(station_state): State<Arc<StationHttpState>>,
    headers: axum::http::HeaderMap,
) -> Result<impl axum::response::IntoResponse, (axum::http::StatusCode, String)> {
    use crate::station_api::events::EventSubscription;
    use axum::response::sse::Event;
    let cursor = parse_event_cursor(&headers)?;
    let (initial, mut updates) = match cursor {
        None => {
            let SnapshotSubscription {
                sequence,
                run_sequence,
                items,
                updates,
            } = station_state.subscribe_with_snapshot().ok_or_else(|| {
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Station snapshot is not ready".to_owned(),
                )
            })?;
            let event = Event::default()
                .event("snapshot")
                .id(format!("{}:{sequence}", station_state.instance_id))
                .json_data(hebbian_contracts::StationEventSnapshot {
                    station_instance_id: station_state.instance_id.clone(),
                    sequence,
                    run_sequence,
                    items: items.iter().map(AsRef::as_ref).collect(),
                })
                .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
            (vec![event], updates)
        }
        Some((instance, sequence)) if instance == station_state.instance_id => {
            match station_state.subscribe_after(sequence) {
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
    State(station_state): State<Arc<StationHttpState>>,
) -> Json<hebbian_contracts::StationInstance> {
    Json(hebbian_contracts::StationInstance {
        id: station_state.instance_id.clone(),
        supports_operation_completion: Some(true),
    })
}

async fn acquire_lease(
    State(station_state): State<Arc<StationHttpState>>,
    Json(input): Json<hebbian_contracts::LeaseAcquireRequest>,
) -> Result<Json<hebbian_contracts::LeaseGrant>, (StatusCode, String)> {
    use crate::station_api::submissions::LeaseReservation;

    let id = station_state.validate_submission_id(&input.station_instance_id, &input.request_id)?;
    let links = station_state
        .arms()
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
                            super::interlocks::manual_control_ready_without_waiting(&links)?;
                        }
                        let mode = if input.take {
                            crate::lease::AcquireMode::Takeover
                        } else {
                            crate::lease::AcquireMode::WaitOrFail
                        };
                        link.acquire_lease(input.request, mode)
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
    station_state: Arc<StationHttpState>,
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
        .arms()
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
                    crate::station_api::commands::preview_answer(&arms, &arm, &candidate.pose);
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

async fn list_traces(State(station_state): State<Arc<StationHttpState>>) -> Response {
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
    State(station_state): State<Arc<StationHttpState>>,
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
    use super::{HttpServer, ServerMessage, StationHttpState, stream_events};
    use axum::extract::State;
    use axum::http::{HeaderMap, HeaderValue, StatusCode};
    use std::sync::Arc;

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

    fn station() -> Arc<StationHttpState> {
        Arc::new(StationHttpState::new(
            &tokio::runtime::Handle::current(),
            Arc::default(),
            vec![],
            Arc::new(crate::station_api::telemetry::DiscardTelemetry),
        ))
    }

    #[test]
    fn http_stream_resumes_after_the_delivered_event() {
        let server = HttpServer::start(
            "127.0.0.1:0",
            vec![],
            Arc::new(crate::station_api::telemetry::DiscardTelemetry),
            Arc::default(),
            |_, _| Ok(axum::Router::new()),
        )
        .unwrap();
        server.station_state.publish_event(ready_state());
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
            server.station_state.resync_arm("replayed".to_owned());
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
        station_state.publish_event(ready_state());
        assert!(
            stream_events(State(Arc::clone(&station_state)), HeaderMap::new())
                .await
                .is_ok()
        );
        station_state.invalidate_station_state();
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

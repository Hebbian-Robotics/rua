use std::collections::BTreeMap;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use hebbian_contracts::{
    ActionSequenceRequest, ActionSequenceStartRequest, CompilationArtifact, CompilationOutcome,
    ProgramContinuation, RunCompiledProgramRequest, SnapshotName,
};

use super::http_state::{SHUTTING_DOWN, StationHttpState};
use super::programs::{
    COMPILATION_SCHEMA_VERSION, CompilationRetention, ProgramArtifact, ProgramCompilation,
};
use super::submissions::{
    OperationId, ProgramReservation, ProgramSubmissionError, ProgramSubmissionRequest,
    SubmissionCompletion, wait_for_outcome,
};

type HttpFailure = (StatusCode, String);

impl From<ProgramSubmissionError> for HttpFailure {
    fn from(error: ProgramSubmissionError) -> Self {
        match error {
            ProgramSubmissionError::Unavailable(detail) => {
                (StatusCode::SERVICE_UNAVAILABLE, detail.into())
            }
            ProgramSubmissionError::Cancelled => (
                StatusCode::CONFLICT,
                "Operation was cancelled before admission".into(),
            ),
            ProgramSubmissionError::MissingCompilation => (
                StatusCode::NOT_FOUND,
                "Compilation artifact is unknown or expired; compile the saved source again".into(),
            ),
            ProgramSubmissionError::IncompatibleCompilation => (
                StatusCode::CONFLICT,
                "Compilation configuration changed; compile the saved source again".into(),
            ),
            ProgramSubmissionError::ExecutionRefused(refusal) => {
                (StatusCode::CONFLICT, refusal.to_string())
            }
            ProgramSubmissionError::CompilationRejected(diagnostic) => {
                (StatusCode::CONFLICT, diagnostic.to_string())
            }
            ProgramSubmissionError::Internal(detail) => internal_failure(detail),
        }
    }
}

fn station_links(
    station_state: &StationHttpState,
) -> Result<
    &std::collections::BTreeMap<String, crate::control_link::LinkHandle>,
    ProgramSubmissionError,
> {
    station_state
        .arms()
        .ok_or(ProgramSubmissionError::Unavailable(
            "Station arms are not ready",
        ))
}

fn internal_failure(error: impl std::fmt::Display) -> HttpFailure {
    (StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
}

pub(crate) async fn compile_action_sequence(
    State(station_state): State<Arc<StationHttpState>>,
    Json(source): Json<ActionSequenceRequest>,
) -> Result<Response, HttpFailure> {
    tokio::task::spawn_blocking(move || {
        let result = station_state
            .compile_program(source, CompilationRetention::WithInspectionFiles)
            .map_err(internal_failure)?;
        Ok(match result {
            ProgramCompilation::Ready(artifact) => Json(CompilationOutcome::Compiled {
                artifact: &artifact.compilation,
            })
            .into_response(),
            ProgramCompilation::Rejected(diagnostic) => {
                Json(CompilationOutcome::<&CompilationArtifact>::Rejected {
                    diagnostics: vec![diagnostic],
                })
                .into_response()
            }
        })
    })
    .await
    .map_err(internal_failure)?
}

pub(crate) async fn import_compilation(
    State(station_state): State<Arc<StationHttpState>>,
    Json(artifact): Json<serde_json::Value>,
) -> Result<Response, HttpFailure> {
    if artifact
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        != Some(u64::from(COMPILATION_SCHEMA_VERSION))
    {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            format!(
                "Unsupported compilation schema. Compile the sequence using the current action schema to produce schema version {COMPILATION_SCHEMA_VERSION}."
            ),
        ));
    }
    let artifact = serde_json::from_value::<CompilationArtifact>(artifact)
        .map_err(|error| (StatusCode::UNPROCESSABLE_ENTITY, error.to_string()))?;
    compile_action_sequence(State(station_state), Json(artifact.source)).await
}

pub(crate) async fn inspect_compilation(
    State(station_state): State<Arc<StationHttpState>>,
    Path(id): Path<String>,
) -> Result<Response, HttpFailure> {
    tokio::task::spawn_blocking(move || {
        station_state
            .lookup_compilation(&id)
            .map(|artifact| Json(&artifact.compilation).into_response())
            .map_err(HttpFailure::from)
    })
    .await
    .map_err(internal_failure)?
}

pub(crate) async fn execute_source(
    State(station_state): State<Arc<StationHttpState>>,
    Json(request): Json<ActionSequenceStartRequest>,
) -> Result<Json<String>, HttpFailure> {
    submit_program(station_state, ProgramSubmissionRequest::Source(request)).await
}

pub(crate) async fn run_program(
    State(station_state): State<Arc<StationHttpState>>,
    Json(request): Json<RunCompiledProgramRequest>,
) -> Result<Json<String>, HttpFailure> {
    submit_program(station_state, ProgramSubmissionRequest::Compiled(request)).await
}

async fn submit_program(
    station_state: Arc<StationHttpState>,
    request: ProgramSubmissionRequest,
) -> Result<Json<String>, HttpFailure> {
    let (instance_id, request_id) = request.submission_identity();
    station_state.require_station_instance(instance_id)?;
    let id = super::parse_operation_id(request_id)?;
    // INVARIANT: Start the worker in the reservation decision so a lost HTTP caller
    // cannot leave the reserved operation without a worker.
    let response = station_state
        .admission
        .clone()
        .decide(move |state| {
            let response = match state.operations.reserve_program(id, request.clone()) {
                ProgramReservation::Existing(outcome) => outcome,
                ProgramReservation::New {
                    outcome,
                    cancellation,
                } => SubmissionCompletion::start(
                    outcome,
                    ProgramSubmissionError::Internal(
                        "Program worker failed; outcome is unknown".into(),
                    ),
                    |completion| {
                        tokio::spawn(async move {
                            completion.publish(
                                execute_submission(station_state, id, request, cancellation).await,
                            );
                        });
                    },
                ),
                ProgramReservation::Cancelled => {
                    return Err(ProgramSubmissionError::Cancelled.into());
                }
                ProgramReservation::Conflict => {
                    return Err((
                        StatusCode::CONFLICT,
                        "Request identifier already belongs to different work".into(),
                    ));
                }
                ProgramReservation::Expired => {
                    return Err((
                        StatusCode::GONE,
                        "Submission outcome expired; this request will not be executed again"
                            .into(),
                    ));
                }
                ProgramReservation::Full => {
                    return Err((
                        StatusCode::SERVICE_UNAVAILABLE,
                        "Too many submissions are pending".into(),
                    ));
                }
            };
            Ok(response)
        })
        .await
        .map_err(|_| ProgramSubmissionError::Unavailable("Station admission is unavailable"))??;
    wait_for_outcome(response)
        .await
        .map_err(|_| internal_failure("Program submission worker stopped"))?
        .map(Json)
        .map_err(HttpFailure::from)
}

async fn execute_submission(
    station_state: Arc<StationHttpState>,
    operation_id: OperationId,
    request: ProgramSubmissionRequest,
    cancellation: tokio::sync::watch::Sender<bool>,
) -> Result<String, ProgramSubmissionError> {
    check_admission(&station_state)?;
    let links = station_links(&station_state)?.clone();
    let worker_station = Arc::clone(&station_state);
    let prepared =
        tokio::task::spawn_blocking(move || prepare_submission(&worker_station, request))
            .await
            .map_err(|error| ProgramSubmissionError::Internal(error.to_string()))??;
    let owner = station_state.admission.clone();
    station_state
        .admission
        .clone()
        .decide(move |state| {
            check_admission(&station_state)?;
            if *cancellation.borrow() {
                return Err(ProgramSubmissionError::Cancelled);
            }
            let id = operation_id.canonical_string();
            let progress = super::runtime::run_compiled_program(
                owner,
                &mut state.executions,
                &links,
                super::runtime::ProgramStart {
                    id: id.clone(),
                    token: prepared.lease_token,
                    cancellation,
                    artifact: prepared.artifact,
                    expected_revisions: prepared.expected_revisions,
                    continuation: prepared.continuation,
                },
            )
            .map_err(ProgramSubmissionError::ExecutionRefused)?;
            state.operations.track_program(operation_id, &progress);
            let updates = progress.subscribe();
            let record = updates.borrow().clone();
            state.publish_execution(record);
            if !prepared.capture_cameras.is_empty() {
                station_state.sequence_images.start(
                    id.clone(),
                    updates,
                    prepared.capture_cameras,
                    station_state.sequence_cameras.get().cloned(),
                );
            }
            Ok(id)
        })
        .await
        .map_err(|_| ProgramSubmissionError::Unavailable("Station admission is unavailable"))?
}

struct PreparedProgram {
    lease_token: String,
    artifact: Arc<ProgramArtifact>,
    expected_revisions: BTreeMap<String, u64>,
    continuation: Option<ProgramContinuation>,
    capture_cameras: Vec<SnapshotName>,
}

fn prepare_submission(
    station_state: &StationHttpState,
    request: ProgramSubmissionRequest,
) -> Result<PreparedProgram, ProgramSubmissionError> {
    Ok(match request {
        ProgramSubmissionRequest::Source(request) => {
            let artifact = match station_state
                .compile_program(request.action_sequence, CompilationRetention::InMemory)
                .map_err(|error| ProgramSubmissionError::Internal(format!("{error:#}")))?
            {
                ProgramCompilation::Ready(artifact) => artifact,
                ProgramCompilation::Rejected(diagnostic) => {
                    return Err(ProgramSubmissionError::CompilationRejected(diagnostic));
                }
            };
            let expected_revisions = artifact.compilation.source.expected_revisions.clone();
            PreparedProgram {
                lease_token: request.lease_token,
                artifact,
                expected_revisions,
                continuation: None,
                capture_cameras: request.capture_cameras,
            }
        }
        ProgramSubmissionRequest::Compiled(request) => {
            let artifact = station_state.lookup_compilation(&request.compilation_id)?;
            PreparedProgram {
                lease_token: request.lease_token,
                artifact,
                expected_revisions: request.expected_revisions,
                continuation: request.continuation,
                capture_cameras: Vec::new(),
            }
        }
    })
}

fn check_admission(station_state: &StationHttpState) -> Result<(), ProgramSubmissionError> {
    if station_state.is_shutting_down() {
        return Err(ProgramSubmissionError::Unavailable(SHUTTING_DOWN));
    }
    Ok(())
}

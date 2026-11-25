use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use hebbian_contracts::{
    ActionSequenceRequest, ActionSequenceStartRequest, CompilationArtifact, CompilationOutcome,
    RunCompiledProgramRequest,
};

use super::http_state::{SHUTTING_DOWN, StationHttpState};
use super::programs::{
    ArtifactLookup, COMPILATION_SCHEMA_VERSION, CompilationRetention, ProgramCompilation,
};
use super::submissions::{
    ProgramSubmissionError, ProgramSubmissionRequest, RequestReservation, SubmissionCompletion,
    wait_for_outcome,
};

type HttpFailure = (StatusCode, String);

impl From<ProgramSubmissionError> for HttpFailure {
    fn from(error: ProgramSubmissionError) -> Self {
        match error {
            ProgramSubmissionError::Unavailable(detail) => {
                (StatusCode::SERVICE_UNAVAILABLE, detail.into())
            }
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
        let targets = station_state
            .compilation_targets()
            .map_err(internal_failure)?;
        let result = station_state
            .compiler
            .compile(
                &station_state.instance_id,
                targets,
                source,
                CompilationRetention::WithInspectionFiles,
            )
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
        lookup_artifact(&station_state, &id)
            .map(|artifact| Json(&artifact.compilation).into_response())
            .map_err(HttpFailure::from)
    })
    .await
    .map_err(internal_failure)?
}

fn lookup_artifact(
    station_state: &StationHttpState,
    id: &str,
) -> Result<Arc<super::programs::ProgramArtifact>, ProgramSubmissionError> {
    let targets = station_state
        .compilation_targets()
        .map_err(ProgramSubmissionError::Internal)?;
    match station_state.compiler.lookup(id, targets) {
        ArtifactLookup::Found(artifact) => Ok(artifact),
        ArtifactLookup::Missing => Err(ProgramSubmissionError::MissingCompilation),
        ArtifactLookup::Incompatible => Err(ProgramSubmissionError::IncompatibleCompilation),
    }
}

pub(crate) async fn execute_source(
    State(station_state): State<Arc<StationHttpState>>,
    Json(request): Json<ActionSequenceStartRequest>,
) -> Result<Json<String>, HttpFailure> {
    let request_id = request.request_id.clone();
    let instance_id = request.station_instance_id.clone();
    submit_program(
        station_state,
        &request_id,
        &instance_id,
        ProgramSubmissionRequest::Source(request),
    )
    .await
}

pub(crate) async fn run_program(
    State(station_state): State<Arc<StationHttpState>>,
    Json(request): Json<RunCompiledProgramRequest>,
) -> Result<Json<String>, HttpFailure> {
    let request_id = request.request_id.clone();
    let instance_id = request.station_instance_id.clone();
    submit_program(
        station_state,
        &request_id,
        &instance_id,
        ProgramSubmissionRequest::Compiled(request),
    )
    .await
}

async fn submit_program(
    station_state: Arc<StationHttpState>,
    request_id: &str,
    instance_id: &str,
    request: ProgramSubmissionRequest,
) -> Result<Json<String>, HttpFailure> {
    let request_id = station_state.validate_submission_id(instance_id, request_id)?;
    let response = match station_state
        .program_submissions
        .reserve(request_id, request.clone())
    {
        RequestReservation::Existing(outcome) => outcome,
        RequestReservation::New(outcome) => SubmissionCompletion::start(
            outcome,
            ProgramSubmissionError::Internal("Program worker failed; outcome is unknown".into()),
            |completion| {
                tokio::spawn(async move {
                    completion.publish(execute_submission(station_state, request).await);
                });
            },
        ),
        RequestReservation::Conflict => {
            return Err((
                StatusCode::CONFLICT,
                "Request identifier already belongs to different work".into(),
            ));
        }
        RequestReservation::Expired => {
            return Err((
                StatusCode::GONE,
                "Submission outcome expired; this request will not be executed again".into(),
            ));
        }
        RequestReservation::Full => {
            return Err((
                StatusCode::SERVICE_UNAVAILABLE,
                "Too many submissions are pending".into(),
            ));
        }
    };
    wait_for_outcome(response)
        .await
        .map_err(|_| internal_failure("Program submission worker stopped"))?
        .map(Json)
        .map_err(HttpFailure::from)
}

async fn execute_submission(
    station_state: Arc<StationHttpState>,
    request: ProgramSubmissionRequest,
) -> Result<String, ProgramSubmissionError> {
    check_admission(&station_state)?;
    let links = station_links(&station_state)?.clone();
    let worker_station = Arc::clone(&station_state);
    let (token, artifact, expected_revisions, continuation) =
        tokio::task::spawn_blocking(move || prepare_submission(&worker_station, request))
            .await
            .map_err(|error| ProgramSubmissionError::Internal(error.to_string()))??;
    let owner = station_state.admission.clone();
    station_state
        .admission
        .clone()
        .decide(move |state| {
            check_admission(&station_state)?;
            let id = super::runtime::run_compiled_program(
                owner,
                &mut state.executions,
                &links,
                token,
                artifact,
                expected_revisions,
                continuation,
            )
            .map_err(ProgramSubmissionError::ExecutionRefused)?;
            if let Some(record) = state.executions.lookup(&id) {
                state.publish_execution(record);
            }
            Ok(id)
        })
        .await
        .map_err(|_| ProgramSubmissionError::Unavailable("Station admission is unavailable"))?
}

type PreparedProgram = (
    String,
    Arc<super::programs::ProgramArtifact>,
    std::collections::BTreeMap<String, u64>,
    Option<hebbian_contracts::ProgramContinuation>,
);

fn prepare_submission(
    station_state: &StationHttpState,
    request: ProgramSubmissionRequest,
) -> Result<PreparedProgram, ProgramSubmissionError> {
    Ok(match request {
        ProgramSubmissionRequest::Source(request) => {
            let targets = station_state
                .compilation_targets()
                .map_err(ProgramSubmissionError::Internal)?;
            let artifact = match station_state
                .compiler
                .compile(
                    &station_state.instance_id,
                    targets,
                    request.action_sequence,
                    CompilationRetention::InMemory,
                )
                .map_err(|error| ProgramSubmissionError::Internal(format!("{error:#}")))?
            {
                ProgramCompilation::Ready(artifact) => artifact,
                ProgramCompilation::Rejected(diagnostic) => {
                    return Err(ProgramSubmissionError::CompilationRejected(diagnostic));
                }
            };
            let expected_revisions = artifact.compilation.source.expected_revisions.clone();
            (request.lease_token, artifact, expected_revisions, None)
        }
        ProgramSubmissionRequest::Compiled(request) => {
            let artifact = lookup_artifact(station_state, &request.compilation_id)?;
            (
                request.lease_token,
                artifact,
                request.expected_revisions,
                request.continuation,
            )
        }
    })
}

fn check_admission(station_state: &StationHttpState) -> Result<(), ProgramSubmissionError> {
    if station_state.is_shutting_down() {
        return Err(ProgramSubmissionError::Unavailable(SHUTTING_DOWN));
    }
    Ok(())
}

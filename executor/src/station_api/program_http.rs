use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use hebbian_contracts::{
    ActionSequenceRequest, ActionSequenceStartRequest, CompilationArtifact, CompilationOutcome,
    RunCompiledProgramRequest,
};

use super::http::StationState;
use super::programs::{ArtifactLookup, ProgramCompilation};
use super::submissions::{ProgramReservation, ProgramSubmissionRequest};

type HttpFailure = (StatusCode, String);

fn station_links(
    station_state: &StationState,
) -> Result<&std::collections::BTreeMap<String, crate::control_link::LinkHandle>, HttpFailure> {
    station_state.arms.get().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "Station arms are not ready".into(),
    ))
}

fn internal_failure(error: impl std::fmt::Display) -> HttpFailure {
    (StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
}

pub(crate) async fn compile_action_sequence(
    State(station_state): State<Arc<StationState>>,
    Json(source): Json<ActionSequenceRequest>,
) -> Result<Json<CompilationOutcome>, HttpFailure> {
    tokio::task::spawn_blocking(move || {
        let targets = station_state
            .compilation_targets()
            .map_err(internal_failure)?;
        let result = station_state
            .compiler
            .compile(&station_state.instance_id, targets, source)
            .map_err(internal_failure)?;
        Ok(Json(match result {
            ProgramCompilation::Ready(artifact) => CompilationOutcome::Compiled {
                artifact: Box::new(artifact.inspection()),
            },
            ProgramCompilation::Rejected(diagnostic) => CompilationOutcome::Rejected {
                diagnostics: vec![diagnostic],
            },
        }))
    })
    .await
    .map_err(internal_failure)?
}

pub(crate) async fn import_compilation(
    State(station_state): State<Arc<StationState>>,
    Json(artifact): Json<CompilationArtifact>,
) -> Result<Json<CompilationOutcome>, HttpFailure> {
    if artifact.schema_version != 1 {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            "Unsupported compilation schema; extract and compile the source explicitly".into(),
        ));
    }
    compile_action_sequence(State(station_state), Json(artifact.source)).await
}

pub(crate) async fn inspect_compilation(
    State(station_state): State<Arc<StationState>>,
    Path(id): Path<String>,
) -> Result<Json<CompilationArtifact>, HttpFailure> {
    tokio::task::spawn_blocking(move || {
        lookup_artifact(&station_state, &id).map(|artifact| Json(artifact.inspection()))
    })
    .await
    .map_err(internal_failure)?
}

fn lookup_artifact(
    station_state: &StationState,
    id: &str,
) -> Result<Arc<super::programs::ProgramArtifact>, HttpFailure> {
    let targets = station_state
        .compilation_targets()
        .map_err(internal_failure)?;
    match station_state.compiler.lookup(id, targets) {
        ArtifactLookup::Found(artifact) => Ok(artifact),
        ArtifactLookup::Missing => Err((
            StatusCode::NOT_FOUND,
            "Compilation artifact is unknown or expired; compile the saved source again".into(),
        )),
        ArtifactLookup::Incompatible => Err((
            StatusCode::CONFLICT,
            "Compilation configuration changed; compile the saved source again".into(),
        )),
    }
}

pub(crate) async fn execute_source(
    State(station_state): State<Arc<StationState>>,
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
    State(station_state): State<Arc<StationState>>,
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
    station_state: Arc<StationState>,
    request_id: &str,
    instance_id: &str,
    request: ProgramSubmissionRequest,
) -> Result<Json<String>, HttpFailure> {
    if instance_id != station_state.instance_id {
        return Err((
            StatusCode::CONFLICT,
            "Station instance changed; inspect current state before submitting again".into(),
        ));
    }
    let request_id = uuid::Uuid::parse_str(request_id)
        .ok()
        .filter(|identifier| identifier.get_version_num() == 4)
        .ok_or((
            StatusCode::BAD_REQUEST,
            "Request identifier must be a UUID v4".into(),
        ))?;
    let mut outcome = match station_state
        .program_submissions
        .reserve(request_id, request.clone())
    {
        ProgramReservation::Existing(outcome) => outcome,
        ProgramReservation::New(outcome) => {
            let receiver = outcome.subscribe();
            tokio::task::spawn_blocking(move || {
                let result = execute_submission(&station_state, request);
                outcome.send_replace(Some(result));
            });
            receiver
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
                "Submission outcome expired; this request will not be executed again".into(),
            ));
        }
        ProgramReservation::Full => {
            return Err((
                StatusCode::SERVICE_UNAVAILABLE,
                "Too many submissions are pending".into(),
            ));
        }
    };
    loop {
        if let Some(result) = outcome.borrow().clone() {
            return result
                .map(Json)
                .map_err(|detail| (StatusCode::CONFLICT, detail));
        }
        outcome
            .changed()
            .await
            .map_err(|_| internal_failure("Program submission worker stopped"))?;
    }
}

fn execute_submission(
    station_state: &Arc<StationState>,
    request: ProgramSubmissionRequest,
) -> Result<String, String> {
    let links = station_links(station_state).map_err(|(_, detail)| detail)?;
    match request {
        ProgramSubmissionRequest::Source(request) => super::runtime::start_action_sequence(
            station_state,
            links,
            Some(request.lease_token),
            request.action_sequence,
        ),
        ProgramSubmissionRequest::Compiled(request) => {
            let artifact = lookup_artifact(station_state, &request.compilation_id)
                .map_err(|(_, detail)| detail)?;
            super::runtime::run_compiled_program(
                station_state,
                links,
                request.lease_token,
                artifact,
                request.expected_revisions,
                request.continuation,
            )
        }
    }
}

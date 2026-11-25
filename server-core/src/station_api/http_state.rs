use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use axum::http::StatusCode;
use hebbian_contracts::CompilationTarget;
use uuid::Uuid;

use crate::control_link::LinkHandle;

use super::admission::AdmissionOwner;
use super::events::EventJournal;
use super::programs::ProgramStore;
use super::submissions::{LeaseSubmissions, ProgramSubmissions};
use super::telemetry::RunTelemetry;

pub(super) const SHUTTING_DOWN: &str = "Station is shutting down";

pub(super) struct StationHttpState {
    shutdown_requested: Arc<AtomicBool>,
    pub(super) program_submissions: ProgramSubmissions,
    pub(super) compiler: ProgramStore,
    compilation_targets: OnceLock<Result<BTreeMap<String, CompilationTarget>, String>>,
    pub(super) trace_directories: Vec<PathBuf>,
    arms: OnceLock<BTreeMap<String, LinkHandle>>,
    pub(super) telemetry: Arc<dyn RunTelemetry>,
    pub(super) admission: AdmissionOwner,
    pub(super) instance_id: String,
    pub(super) lease_submissions: LeaseSubmissions,
    pub(super) events: Arc<EventJournal>,
}

impl StationHttpState {
    pub(super) fn new(
        runtime: &tokio::runtime::Handle,
        shutdown_requested: Arc<AtomicBool>,
        trace_directories: Vec<PathBuf>,
        telemetry: Arc<dyn RunTelemetry>,
    ) -> Self {
        let events = Arc::new(EventJournal::default());
        Self {
            shutdown_requested,
            program_submissions: ProgramSubmissions::default(),
            compiler: ProgramStore::default(),
            compilation_targets: OnceLock::new(),
            trace_directories,
            arms: OnceLock::new(),
            telemetry,
            admission: AdmissionOwner::start(runtime, Arc::clone(&events)),
            instance_id: Uuid::new_v4().to_string(),
            lease_submissions: LeaseSubmissions::default(),
            events,
        }
    }

    pub(super) fn is_shutting_down(&self) -> bool {
        self.shutdown_requested.load(Ordering::Acquire)
    }

    pub(super) fn attach_arms(&self, arms: &BTreeMap<String, LinkHandle>) -> bool {
        self.arms.set(arms.clone()).is_ok()
    }

    pub(super) fn arms(&self) -> Option<&BTreeMap<String, LinkHandle>> {
        self.arms.get()
    }

    pub(super) fn compilation_targets(
        &self,
    ) -> Result<&BTreeMap<String, CompilationTarget>, String> {
        let arms = self.arms().ok_or("Station arms are not ready")?;
        self.compilation_targets
            .get_or_init(|| {
                super::programs::station_targets(arms).map_err(|error| error.to_string())
            })
            .as_ref()
            .map_err(Clone::clone)
    }

    // INVARIANT: Every arm shares the station lease; any ready arm can handle lease operations.
    pub(super) fn lease_link(&self) -> Result<&LinkHandle, (StatusCode, String)> {
        self.arms().and_then(|arms| arms.values().next()).ok_or((
            StatusCode::SERVICE_UNAVAILABLE,
            "Station arms are not ready".to_owned(),
        ))
    }

    pub(super) fn require_station_instance(
        &self,
        station_instance_id: &str,
    ) -> Result<(), (StatusCode, String)> {
        if station_instance_id != self.instance_id {
            return Err((
                StatusCode::CONFLICT,
                "Station instance changed; request outcome is unknown".to_owned(),
            ));
        }
        Ok(())
    }

    pub(super) fn validate_submission_id(
        &self,
        station_instance_id: &str,
        request_id: &str,
    ) -> Result<Uuid, (StatusCode, String)> {
        self.require_station_instance(station_instance_id)?;
        let id = Uuid::parse_str(request_id).map_err(|_| {
            (
                StatusCode::BAD_REQUEST,
                "Expected a UUID v4 request ID".to_owned(),
            )
        })?;
        if id.get_version_num() != 4 || id.get_variant() != uuid::Variant::RFC4122 {
            return Err((
                StatusCode::BAD_REQUEST,
                "Expected a UUID v4 request ID".to_owned(),
            ));
        }
        Ok(id)
    }
}

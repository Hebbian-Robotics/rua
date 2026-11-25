use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use axum::http::StatusCode;
use hebbian_contracts::{
    ActionSequenceRequest, CompilationTarget, RunEvent, ServerMessage, TraceSpan,
};
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::control_link::LinkHandle;

use super::admission::AdmissionOwner;
use super::events::{EventJournal, EventSubscription, StationEvent, StreamPayload};
use super::programs::{
    ArtifactLookup, CompilationRetention, ProgramArtifact, ProgramCompilation, ProgramStore,
};
use super::submissions::{LeaseSubmissions, ProgramSubmissionError, ProgramSubmissions};
use super::telemetry::RunTelemetry;

pub(super) const SHUTTING_DOWN: &str = "Station is shutting down";

pub(super) struct SnapshotSubscription {
    pub(super) sequence: u64,
    pub(super) run_sequence: u64,
    pub(super) items: Vec<Arc<StreamPayload>>,
    pub(super) updates: broadcast::Receiver<StationEvent>,
}

pub(super) struct StationHttpState {
    shutdown_requested: Arc<AtomicBool>,
    pub(super) sequence_cameras: OnceLock<super::SequenceCameraSource>,
    pub(super) sequence_images: super::sequence_images::SequenceImages,
    pub(super) program_submissions: ProgramSubmissions,
    programs: ProgramStore,
    compilation_targets: OnceLock<Result<BTreeMap<String, CompilationTarget>, String>>,
    pub(super) trace_directories: Vec<PathBuf>,
    arms: OnceLock<BTreeMap<String, LinkHandle>>,
    telemetry: Arc<dyn RunTelemetry>,
    pub(super) admission: AdmissionOwner,
    pub(super) instance_id: String,
    pub(super) lease_submissions: LeaseSubmissions,
    events: Arc<EventJournal>,
}

impl StationHttpState {
    pub(super) fn new(
        runtime: &tokio::runtime::Handle,
        shutdown_requested: Arc<AtomicBool>,
        trace_directories: Vec<PathBuf>,
        telemetry: Arc<dyn RunTelemetry>,
    ) -> Self {
        let events = Arc::new(EventJournal::default());
        let sequence_images = super::sequence_images::SequenceImages::default();
        sequence_images.start_cleanup(runtime);
        Self {
            shutdown_requested,
            sequence_cameras: OnceLock::new(),
            sequence_images,
            program_submissions: ProgramSubmissions::default(),
            programs: ProgramStore::default(),
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

    pub(super) fn attach_arms(
        self: &Arc<Self>,
        arms: BTreeMap<String, LinkHandle>,
        runtime: Option<&tokio::runtime::Handle>,
    ) {
        if self.arms.set(arms.clone()).is_err() {
            return;
        }
        let Some(runtime) = runtime else { return };
        let snapshot_arms = arms.clone();
        let snapshot_state = Arc::clone(self);
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
                    snapshot_state.publish_event(ServerMessage::State {
                        state: Box::new(state),
                    });
                } else {
                    snapshot_state.invalidate_station_state();
                }
            }
        });
        let run_state = Arc::clone(self);
        runtime.spawn(async move {
            let mut cursor = run_state.telemetry.newest_seq();
            let mut revision = None;
            let mut interval = tokio::time::interval(Duration::from_millis(100));
            loop {
                interval.tick().await;
                let updates = run_state.telemetry.updates(&mut cursor, &mut revision);
                for record in updates.records {
                    run_state.publish_event(record);
                }
                if let Some(run) = updates.snapshot {
                    run_state.publish_event(ServerMessage::Run { run: Box::new(run) });
                }
            }
        });
        if let Some(lease_link) = arms.values().next().cloned() {
            let admission = self.admission.clone();
            runtime.spawn(async move {
                let mut interval = tokio::time::interval(Duration::from_millis(100));
                loop {
                    interval.tick().await;
                    let lease_link = lease_link.clone();
                    if admission
                        .decide(move |_| lease_link.tick_lease())
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
            });
        }
        for (arm_name, link) in arms {
            let arm_state = Arc::clone(self);
            let event_link = link.clone();
            runtime.spawn(async move {
                loop {
                    let (client, mut frames) = event_link.connect(4096);
                    event_link.snapshot(client);
                    while let Some(frame) = frames.recv().await {
                        arm_state.publish_event(ServerMessage::Frame {
                            arm: arm_name.clone(),
                            frame: Box::new(frame),
                        });
                    }
                    event_link.disconnect(client);
                    arm_state.resync_arm(arm_name.clone());
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            });
            runtime.spawn(crate::station_api::interlocks::watch_arm(
                link,
                self.admission.clone(),
            ));
        }
    }

    pub(super) fn publish_event(&self, message: ServerMessage) {
        self.events.publish(message);
    }

    pub(super) fn invalidate_station_state(&self) {
        self.events.invalidate_station_state();
    }

    pub(super) fn resync_arm(&self, arm: String) {
        self.events.resync_arm(arm);
    }

    pub(super) fn subscribe_with_snapshot(&self) -> Option<SnapshotSubscription> {
        let (sequence, items, updates) = self.events.subscribe_with_snapshot();
        if !items.iter().any(|event| event.is_station_state()) {
            return None;
        }
        Some(SnapshotSubscription {
            sequence,
            run_sequence: self.telemetry.newest_seq(),
            items,
            updates,
        })
    }

    pub(super) fn subscribe_after(&self, sequence: u64) -> EventSubscription {
        self.events.subscribe_after(sequence)
    }

    pub(super) fn record_run_event(&self, event: RunEvent) {
        self.telemetry.record_event(event);
    }

    pub(super) fn record_run_spans(&self, spans: Vec<TraceSpan>) {
        self.telemetry.record_spans(spans);
    }

    pub(super) fn replay_run_records(&self, mut after: u64) -> Vec<ServerMessage> {
        self.telemetry.updates(&mut after, &mut None).records
    }

    pub(super) fn arms(&self) -> Option<&BTreeMap<String, LinkHandle>> {
        self.arms.get()
    }

    pub(super) fn compile_program(
        &self,
        source: ActionSequenceRequest,
        retention: CompilationRetention,
    ) -> anyhow::Result<ProgramCompilation> {
        let targets = self.compilation_targets().map_err(anyhow::Error::msg)?;
        self.programs
            .compile(&self.instance_id, targets, source, retention)
    }

    pub(super) fn lookup_compilation(
        &self,
        id: &str,
    ) -> Result<Arc<ProgramArtifact>, ProgramSubmissionError> {
        let targets = self
            .compilation_targets()
            .map_err(ProgramSubmissionError::Internal)?;
        match self.programs.lookup(id, targets) {
            ArtifactLookup::Found(artifact) => Ok(artifact),
            ArtifactLookup::Missing => Err(ProgramSubmissionError::MissingCompilation),
            ArtifactLookup::Incompatible => Err(ProgramSubmissionError::IncompatibleCompilation),
        }
    }

    fn compilation_targets(&self) -> Result<&BTreeMap<String, CompilationTarget>, String> {
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

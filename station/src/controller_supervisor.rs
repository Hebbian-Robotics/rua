use std::collections::BTreeMap;
use std::process::ExitStatus;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    routing::{get, post},
};
use hebbian_rua_core::control_link::Link;
use hebbian_rua_core::controller_bridge::StationSession;
use hebbian_rua_core::controller_service::{ReadyController, serve_station_connection};
use hebbian_rua_core::lease::Lease;

use hebbian_contracts::{ControllerStatus, ReconnectRequest};

use crate::controller_process::{ControllerLaunch, ControllerProcess, PlantSelection};
use crate::recording::RecordingManager;

const POLL_INTERVAL: Duration = Duration::from_millis(50);
const CONTROLLER_CONNECT_TIMEOUT: Duration = Duration::from_secs(60);
const CONTROLLER_EXIT_TIMEOUT: Duration = Duration::from_secs(2);

enum ControllerPhase {
    Unmanaged,
    Starting,
    Online {
        _readiness: ReadyController,
    },
    Stopping,
    Unavailable {
        reason: String,
        guard_released: bool,
        reports: Vec<String>,
    },
    WaitingForGuard {
        reason: String,
        reports: Vec<String>,
    },
}

struct GuardObservation {
    released: bool,
    reports: Option<Vec<String>>,
}

impl GuardObservation {
    fn read(process: &mut ControllerProcess) -> Self {
        Self {
            released: process.guard_released(),
            reports: process.take_updated_reports(),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum LaunchDecision {
    Wait,
    Start,
}

#[deny(clippy::disallowed_methods)]
impl ControllerPhase {
    fn is_available(&self) -> bool {
        matches!(self, Self::Online { .. } | Self::Unmanaged)
    }

    fn reconnect_requested(&self) -> Result<Self, &'static str> {
        match self {
            Self::Unavailable {
                reason, reports, ..
            } => Ok(Self::WaitingForGuard {
                reason: reason.clone(),
                reports: reports.clone(),
            }),
            Self::Online { .. } => Err("The controller is already online"),
            Self::Unmanaged => Err("The station does not supervise a controller"),
            Self::Stopping => Err("The controller is stopping"),
            Self::Starting | Self::WaitingForGuard { .. } => {
                Err("Controller startup or recovery is already in progress")
            }
        }
    }

    fn poll_launch(&mut self, previous: Option<GuardObservation>) -> LaunchDecision {
        match self {
            Self::Starting => LaunchDecision::Start,
            Self::Unavailable {
                guard_released,
                reports,
                ..
            } => {
                if let Some(observation) = previous {
                    *guard_released = observation.released;
                    if let Some(updated) = observation.reports {
                        *reports = updated;
                    }
                }
                LaunchDecision::Wait
            }
            Self::WaitingForGuard { reports, .. } => {
                if let Some(observation) = previous {
                    if let Some(updated) = observation.reports {
                        *reports = updated;
                    }
                    if !observation.released {
                        return LaunchDecision::Wait;
                    }
                }
                *self = Self::Starting;
                LaunchDecision::Start
            }
            Self::Online { .. } | Self::Stopping | Self::Unmanaged => LaunchDecision::Wait,
        }
    }

    fn status(&self) -> ControllerStatus {
        match self {
            Self::Unmanaged => ControllerStatus::Unmanaged,
            Self::Starting => ControllerStatus::Starting,
            Self::Online { .. } => ControllerStatus::Online,
            Self::Stopping => ControllerStatus::Stopping,
            Self::Unavailable {
                reason,
                guard_released,
                reports,
            } => ControllerStatus::Unavailable {
                reason: reason.clone(),
                guard_released: *guard_released,
                reports: reports.clone(),
            },
            Self::WaitingForGuard { reason, reports } => ControllerStatus::WaitingForGuard {
                reason: reason.clone(),
                reports: reports.clone(),
            },
        }
    }
}

#[derive(Clone)]
pub struct ControllerSupervisor {
    phase: Arc<Mutex<ControllerPhase>>,
    wake_worker: mpsc::Sender<()>,
    recording: Arc<Mutex<Option<RecordingManager>>>,
}

impl ControllerSupervisor {
    #[must_use]
    pub fn unmanaged() -> Self {
        let (wake_worker, _receiver) = mpsc::channel();
        Self {
            phase: Arc::new(Mutex::new(ControllerPhase::Unmanaged)),
            wake_worker,
            recording: Arc::new(Mutex::new(None)),
        }
    }

    #[must_use]
    pub fn status(&self) -> ControllerStatus {
        self.phase
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .status()
    }

    pub fn availability_reader(&self) -> impl Fn() -> bool + Send + Sync + 'static + use<> {
        let phase = Arc::clone(&self.phase);
        move || {
            phase
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_available()
        }
    }

    fn set_phase(&self, phase: ControllerPhase) {
        let available = phase.is_available();
        *self
            .phase
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = phase;
        // INVARIANT: Recording reads availability under its state lock, so release the
        // phase lock before marking an interruption.
        if !available {
            let recording = self
                .recording
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            if let Some(recording) = recording {
                recording.mark_controller_interruption();
            }
        }
    }

    pub fn attach_recording(&self, recording: RecordingManager) {
        *self
            .recording
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(recording);
    }

    pub fn reconnect(&self) -> Result<(), &'static str> {
        let mut status = self
            .phase
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let next = status.reconnect_requested()?;
        self.wake_worker
            .send(())
            .map_err(|_| "The controller supervisor stopped")?;
        *status = next;
        Ok(())
    }
}

// INVARIANT: Keep every handoff fenced until fresh controller feedback confirms readiness.
#[must_use]
pub fn start(
    configuration: ControllerLaunch,
    session: StationSession,
    links: BTreeMap<String, Arc<Link>>,
    lease: Arc<Lease>,
    shutdown: Arc<AtomicBool>,
) -> (ControllerSupervisor, JoinHandle<()>) {
    let (wake_worker, wakeups) = mpsc::channel();
    let supervisor = ControllerSupervisor {
        phase: Arc::new(Mutex::new(ControllerPhase::Starting)),
        wake_worker,
        recording: Arc::new(Mutex::new(None)),
    };
    let worker = supervisor.clone();
    lease.controller_lost();
    for link in links.values() {
        link.controller_unavailable("The controller is starting".into());
    }
    let thread = thread::spawn(move || {
        run(
            &worker,
            &configuration,
            session,
            &links,
            &lease,
            &shutdown,
            &wakeups,
        );
    });
    (supervisor, thread)
}

fn start_connection(
    process: &mut ControllerProcess,
    shutdown: &AtomicBool,
    startup_timeout: Duration,
) -> Result<std::os::unix::net::UnixStream, String> {
    let started = Instant::now();
    while !shutdown.load(Ordering::Acquire) {
        if let Some(socket) = process.try_accept().map_err(|error| format!("{error:#}"))? {
            return Ok(socket);
        }
        if let Some(status) = process.try_wait().map_err(|error| format!("{error:#}"))? {
            return Err(format!("Controller exited during startup: {status}"));
        }
        if started.elapsed() >= startup_timeout {
            return Err("Controller startup timed out".into());
        }
        thread::sleep(POLL_INTERVAL);
    }
    Err("Station is shutting down".into())
}

fn wait_for_exit(process: &mut ControllerProcess) -> anyhow::Result<ExitStatus> {
    let deadline = Instant::now() + CONTROLLER_EXIT_TIMEOUT;
    loop {
        if let Some(status) = process.try_wait()? {
            return Ok(status);
        }
        anyhow::ensure!(Instant::now() < deadline, "Controller did not exit");
        thread::sleep(POLL_INTERVAL);
    }
}

fn finish_controller(process: &mut ControllerProcess, result: Result<(), String>) -> String {
    let result = result.and_then(|()| match wait_for_exit(process) {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => Err(format!("Controller exited during shutdown: {status}")),
        Err(error) => Err(format!("Controller shutdown failed: {error:#}")),
    });
    match result {
        Ok(()) => {
            tracing::info!("Controller shut down cleanly");
            "Controller shut down cleanly".into()
        }
        Err(reason) => {
            tracing::warn!(%reason, "Motor controller unavailable");
            if let Err(error) = process
                .stop_controller()
                .and_then(|()| wait_for_exit(process))
            {
                tracing::error!(error = %format_args!("{error:#}"), "Could not stop the controller");
            }
            reason
        }
    }
}

fn run(
    supervisor: &ControllerSupervisor,
    configuration: &ControllerLaunch,
    mut session: StationSession,
    links: &BTreeMap<String, Arc<Link>>,
    lease: &Lease,
    shutdown: &AtomicBool,
    wakeups: &mpsc::Receiver<()>,
) {
    let mut startup_timeout = CONTROLLER_CONNECT_TIMEOUT;
    if matches!(configuration.plant, PlantSelection::Simulator { .. }) {
        // WHY: The child accepts each plant and reads its hello before connecting
        // to the station. Allow those sequential waits on top of normal startup.
        for link in links.values() {
            if let Some(timeouts) = link.handle().config().simulator_timeouts() {
                startup_timeout = startup_timeout
                    .saturating_add(timeouts.connect)
                    .saturating_add(timeouts.reply);
            }
        }
    }
    let mut previous: Option<ControllerProcess> = None;
    while !shutdown.load(Ordering::Acquire) {
        let observation = previous.as_mut().map(GuardObservation::read);
        let decision = supervisor
            .phase
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .poll_launch(observation);
        if decision == LaunchDecision::Wait {
            match wakeups.recv_timeout(POLL_INTERVAL) {
                Ok(()) | Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        if shutdown.load(Ordering::Acquire) {
            break;
        }
        previous = None;
        let mut process = match ControllerProcess::start(configuration) {
            Ok(process) => process,
            Err(error) => {
                tracing::warn!(error = %format_args!("{error:#}"), "Cannot start controller");
                supervisor.set_phase(ControllerPhase::Unavailable {
                    reason: format!("Cannot start controller: {error:#}"),
                    guard_released: true,
                    reports: Vec::new(),
                });
                continue;
            }
        };
        let connection = start_connection(&mut process, shutdown, startup_timeout);
        let result = connection.and_then(|socket| {
            serve_station_connection(
                socket,
                process.instance(),
                &mut session,
                links,
                shutdown,
                |ready| {
                    ready.restore_motion_authority(lease);
                    for link in links.values() {
                        link.publish_log(hebbian_contracts::LoggedEvent::controller_available());
                    }
                    supervisor.set_phase(ControllerPhase::Online { _readiness: ready });
                },
                || {
                    lease.controller_lost();
                    supervisor.set_phase(ControllerPhase::Stopping);
                },
            )
            .map_err(|error| format!("Controller connection ended: {error}"))
        });
        let reason = finish_controller(&mut process, result);
        lease.controller_lost();
        for link in links.values() {
            link.controller_unavailable(reason.clone());
        }
        let guard_released = process.guard_released();
        // INVARIANT: Publish the recoverable status once so a queued reconnect stays visible.
        supervisor.set_phase(ControllerPhase::Unavailable {
            reason,
            guard_released,
            reports: process.take_updated_reports().unwrap_or_default(),
        });
        previous = Some(process);
    }
}

async fn read_status(State(supervisor): State<ControllerSupervisor>) -> Json<ControllerStatus> {
    Json(supervisor.status())
}

async fn reconnect(
    State(supervisor): State<ControllerSupervisor>,
    Json(request): Json<ReconnectRequest>,
) -> Result<Json<ControllerStatus>, (StatusCode, Json<serde_json::Value>)> {
    if !request.supported {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "detail": "Support the arm before reconnecting" })),
        ));
    }
    supervisor.reconnect().map_err(|reason| {
        (
            StatusCode::CONFLICT,
            Json(serde_json::json!({ "detail": reason })),
        )
    })?;
    Ok(Json(supervisor.status()))
}

pub fn router(supervisor: ControllerSupervisor) -> Router {
    Router::new()
        .route("/controller", get(read_status))
        .route("/controller/reconnect", post(reconnect))
        .with_state(supervisor)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconnect_requires_both_an_operator_request_and_guard_release() {
        let mut phase = ControllerPhase::Unavailable {
            reason: "Controller exited".into(),
            guard_released: false,
            reports: vec![],
        };
        assert_eq!(
            phase.poll_launch(Some(GuardObservation {
                released: true,
                reports: None,
            })),
            LaunchDecision::Wait
        );
        phase = phase.reconnect_requested().unwrap();
        assert_eq!(
            phase.poll_launch(Some(GuardObservation {
                released: false,
                reports: Some(vec!["Releasing motors".into()]),
            })),
            LaunchDecision::Wait
        );
        assert!(
            matches!(phase.status(), ControllerStatus::WaitingForGuard {reports, ..}
            if reports == ["Releasing motors"])
        );
        assert!(phase.reconnect_requested().is_err());
        assert_eq!(
            phase.poll_launch(Some(GuardObservation {
                released: true,
                reports: None,
            })),
            LaunchDecision::Start
        );
        assert!(matches!(phase.status(), ControllerStatus::Starting));
    }

    #[test]
    fn a_failed_spawn_can_be_retried_without_a_previous_process() {
        let mut phase = ControllerPhase::Unavailable {
            reason: "Spawn failed".into(),
            guard_released: true,
            reports: vec![],
        };
        assert_eq!(phase.poll_launch(None), LaunchDecision::Wait);
        phase = phase.reconnect_requested().unwrap();
        assert_eq!(phase.poll_launch(None), LaunchDecision::Start);
    }
}

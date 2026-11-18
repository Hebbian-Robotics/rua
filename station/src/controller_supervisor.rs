use std::collections::BTreeMap;
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
use hebbian_robot_use_server::control_link::Link;
use hebbian_robot_use_server::controller_bridge::StationSession;
use hebbian_robot_use_server::controller_service::serve_station_connection;
use hebbian_robot_use_server::lease::Lease;

use hebbian_contracts::{ControllerStatus, ReconnectRequest};

use crate::controller_process::{ControllerLaunch, ControllerProcess};
use crate::recording::RecordingManager;

const POLL_INTERVAL: Duration = Duration::from_millis(50);
const STARTUP_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Clone)]
pub struct ControllerSupervisor {
    status: Arc<Mutex<ControllerStatus>>,
    requests: mpsc::Sender<()>,
    recording: Arc<Mutex<Option<RecordingManager>>>,
}

impl ControllerSupervisor {
    #[must_use]
    pub fn unmanaged() -> Self {
        let (requests, _receiver) = mpsc::channel();
        Self {
            status: Arc::new(Mutex::new(ControllerStatus::Unmanaged)),
            requests,
            recording: Arc::new(Mutex::new(None)),
        }
    }

    #[must_use]
    pub fn status(&self) -> ControllerStatus {
        self.status
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn set_status(&self, status: ControllerStatus) {
        if let ControllerStatus::Unavailable { reason, .. } = &status {
            eprintln!("Motor controller unavailable: {reason}");
        }
        let mut current = self
            .status
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(recording) = self
            .recording
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
        {
            if matches!(
                status,
                ControllerStatus::Online | ControllerStatus::Unmanaged
            ) {
                recording.controller_ready();
            } else {
                recording.controller_unavailable();
            }
        }
        *current = status;
    }

    pub fn attach_recording(&self, recording: RecordingManager) {
        let status = self
            .status
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut slot = self
            .recording
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if matches!(
            *status,
            ControllerStatus::Online | ControllerStatus::Unmanaged
        ) {
            recording.controller_ready();
        } else {
            recording.controller_unavailable();
        }
        *slot = Some(recording);
    }

    pub fn reconnect(&self) -> Result<(), &'static str> {
        let mut status = self
            .status
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let ControllerStatus::Unavailable {
            reason, reports, ..
        } = &*status
        else {
            return Err("The controller is online or recovery is already in progress");
        };
        let previous = status.clone();
        *status = ControllerStatus::WaitingForGuard {
            reason: reason.clone(),
            reports: reports.clone(),
        };
        if self.requests.send(()).is_err() {
            *status = previous;
            return Err("The controller supervisor stopped");
        }
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
    let (requests, incoming) = mpsc::channel();
    let supervisor = ControllerSupervisor {
        status: Arc::new(Mutex::new(ControllerStatus::Starting)),
        requests,
        recording: Arc::new(Mutex::new(None)),
    };
    let worker = supervisor.clone();
    lease.controller_lost();
    for link in links.values() {
        link.controller_unavailable("The controller is starting".into());
    }
    let thread = thread::spawn(move || {
        run(
            worker,
            configuration,
            session,
            links,
            lease,
            shutdown,
            incoming,
        );
    });
    (supervisor, thread)
}

fn refresh_guard_status(
    supervisor: &ControllerSupervisor,
    process: &mut ControllerProcess,
) -> bool {
    let released = matches!(process.try_wait(), Ok(Some(_))) && process.report_pipe_closed();
    let mut status = supervisor
        .status
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    match &mut *status {
        ControllerStatus::Unavailable {
            guard_released,
            reports,
            ..
        } => {
            *guard_released = released;
            *reports = process.reports();
        }
        ControllerStatus::WaitingForGuard { reports, .. } => *reports = process.reports(),
        ControllerStatus::Starting | ControllerStatus::Online | ControllerStatus::Unmanaged => {}
    }
    released
}

fn wait_for_release(
    supervisor: &ControllerSupervisor,
    process: &mut ControllerProcess,
    shutdown: &AtomicBool,
) -> bool {
    while !shutdown.load(Ordering::Acquire) {
        if refresh_guard_status(supervisor, process) {
            return true;
        }
        thread::sleep(POLL_INTERVAL);
    }
    false
}

fn start_connection(
    process: &mut ControllerProcess,
    shutdown: &AtomicBool,
) -> Result<std::os::unix::net::UnixStream, String> {
    let deadline = Instant::now() + STARTUP_TIMEOUT;
    while !shutdown.load(Ordering::Acquire) {
        if let Some(socket) = process.try_accept().map_err(|error| format!("{error:#}"))? {
            return Ok(socket);
        }
        if let Some(status) = process.try_wait().map_err(|error| format!("{error:#}"))? {
            return Err(format!("Controller exited during startup: {status}"));
        }
        if Instant::now() >= deadline {
            let _ = process.stop_controller();
            return Err("Controller startup timed out".into());
        }
        thread::sleep(POLL_INTERVAL);
    }
    let _ = process.stop_controller();
    Err("Station is shutting down".into())
}

fn run(
    supervisor: ControllerSupervisor,
    configuration: ControllerLaunch,
    mut session: StationSession,
    links: BTreeMap<String, Arc<Link>>,
    lease: Arc<Lease>,
    shutdown: Arc<AtomicBool>,
    incoming: mpsc::Receiver<()>,
) {
    let mut previous: Option<ControllerProcess> = None;
    let mut start_requested = true;
    while !shutdown.load(Ordering::Acquire) {
        if !start_requested {
            if let Some(process) = previous.as_mut() {
                refresh_guard_status(&supervisor, process);
            }
            if incoming.recv_timeout(POLL_INTERVAL).is_ok() {
                start_requested = true;
            }
            continue;
        }
        if let Some(process) = previous.as_mut()
            && !wait_for_release(&supervisor, process, &shutdown)
        {
            break;
        }
        previous = None;
        supervisor.set_status(ControllerStatus::Starting);
        let mut process = match ControllerProcess::start(&configuration) {
            Ok(process) => process,
            Err(error) => {
                supervisor.set_status(ControllerStatus::Unavailable {
                    reason: format!("Cannot start controller: {error:#}"),
                    guard_released: true,
                    reports: Vec::new(),
                });
                start_requested = false;
                continue;
            }
        };
        let connection = start_connection(&mut process, &shutdown);
        let reason = match connection {
            Ok(socket) => {
                let result = serve_station_connection(
                    socket,
                    process.instance(),
                    &mut session,
                    &links,
                    &shutdown,
                    || {
                        lease.controller_ready();
                        for link in links.values() {
                            link.publish(hebbian_contracts::Event::ControllerAvailable);
                        }
                        supervisor.set_status(ControllerStatus::Online);
                    },
                    || {
                        lease.controller_lost();
                        supervisor.set_status(ControllerStatus::Unavailable {
                            reason: "The controller connection was lost".into(),
                            guard_released: false,
                            reports: Vec::new(),
                        });
                    },
                );
                match result {
                    Ok(()) => "Controller stopped".to_owned(),
                    Err(error) => {
                        if let Err(stop_error) = process.stop_controller() {
                            eprintln!("Could not stop the unresponsive controller: {stop_error:#}");
                        }
                        format!("Controller connection lost: {error}")
                    }
                }
            }
            Err(reason) => reason,
        };
        lease.controller_lost();
        for link in links.values() {
            link.controller_unavailable(reason.clone());
        }
        let guard_released =
            matches!(process.try_wait(), Ok(Some(_))) && process.report_pipe_closed();
        supervisor.set_status(ControllerStatus::Unavailable {
            reason,
            guard_released,
            reports: process.reports(),
        });
        previous = Some(process);
        start_requested = false;
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
        .route("/viewer/api/controller", get(read_status))
        .route("/viewer/api/controller/reconnect", post(reconnect))
        .with_state(supervisor)
}

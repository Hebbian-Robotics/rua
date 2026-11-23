use crate::child_process_group::{ChildProcessGroup, ProcessExit};
use crate::lease_routes::Refusal;
use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    routing::{get, post},
};
use hebbian_contracts::{CONTROLLER_INTERRUPTION_MARKER, LeaseToken, RecordingStatus};
use nix::sys::signal::Signal;
use std::{
    io::Write,
    path::PathBuf,
    process::Stdio,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::{
    process::Command,
    sync::{oneshot, watch},
    time::Duration,
};
pub struct RecordingConfiguration {
    pub credentials: station_access::Credentials,
    pub room: String,
    pub directory: PathBuf,
    pub executable: PathBuf,
}
#[derive(Clone)]
pub struct RecordingManager {
    station_url: String,
    recording: Option<Arc<RecordingConfiguration>>,
    announcements: watch::Sender<RecordingAnnouncement>,
    announced: watch::Receiver<u64>,
    state: Arc<Mutex<RecordingState>>,
    controller_available: Arc<AtomicBool>,
    shutting_down: Arc<AtomicBool>,
    ended: watch::Sender<()>,
}

#[derive(Clone, Copy, Default)]
struct RecordingAnnouncement {
    enabled: bool,
    revision: u64,
}

struct RecordingSession {
    name: String,
    controller_interrupted: bool,
}

enum RecordingState {
    Idle,
    Recording {
        session: RecordingSession,
        stop: oneshot::Sender<()>,
    },
    Stopping(RecordingSession),
    Finished {
        name: String,
    },
    Failed {
        name: String,
        detail: String,
    },
}

impl RecordingState {
    fn active_session(&self) -> Option<&RecordingSession> {
        match self {
            Self::Recording { session, .. } | Self::Stopping(session) => Some(session),
            Self::Idle | Self::Finished { .. } | Self::Failed { .. } => None,
        }
    }

    fn stop(&mut self) {
        *self = match std::mem::replace(self, Self::Idle) {
            Self::Recording { session, stop } => {
                let _ = stop.send(());
                Self::Stopping(session)
            }
            other => other,
        };
    }
}

fn finalized_recording(
    recording_name: String,
    controller_interrupted: bool,
    finalized: std::io::Result<ProcessExit>,
) -> RecordingState {
    let outcome = match finalized {
        Ok(ProcessExit::Requested(status)) if status.success() => Ok(()),
        Ok(ProcessExit::Requested(status)) => Err(format!(
            "Recorder finalized with {status}; inspect the recording status for details"
        )),
        Ok(ProcessExit::Unexpected(status)) => {
            Err(format!("Recorder exited unexpectedly: {status}"))
        }
        Err(error) => Err(error.to_string()),
    };
    match outcome {
        Ok(()) if controller_interrupted => RecordingState::Failed {
            name: recording_name,
            detail: "The motor controller was unavailable during the recording".into(),
        },
        Ok(()) => RecordingState::Finished {
            name: recording_name,
        },
        Err(detail) => RecordingState::Failed {
            name: recording_name,
            detail,
        },
    }
}

impl RecordingManager {
    #[must_use]
    pub fn new(configuration: Option<RecordingConfiguration>, station_url: String) -> Self {
        let recording = configuration.map(Arc::new);
        let (announcements, mut receiver) = watch::channel(RecordingAnnouncement::default());
        let (completed, announced) = watch::channel(0);
        let configuration = recording.clone();
        // INVARIANT: Keep only the latest desired state while a request is in flight.
        // No recording state lock is held during the HTTP calls.
        tokio::spawn(async move {
            while receiver.changed().await.is_ok() {
                let announcement = *receiver.borrow_and_update();
                if let Some(configuration) = &configuration
                    && let Err(error) = configuration
                        .credentials
                        .announce_recording(&configuration.room, announcement.enabled)
                        .await
                {
                    tracing::warn!(%error, "Recording announcement failed");
                }
                completed.send_replace(announcement.revision);
            }
        });
        Self {
            station_url,
            recording,
            announcements,
            announced,
            state: Arc::new(Mutex::new(RecordingState::Idle)),
            controller_available: Arc::new(AtomicBool::new(true)),
            shutting_down: Arc::new(AtomicBool::new(false)),
            ended: watch::Sender::new(()),
        }
    }

    fn mark_interruption_file(&self, name: &str) -> Result<(), String> {
        let configuration = self
            .recording
            .as_ref()
            .ok_or("Recording is not configured")?;
        let directory = configuration.directory.join(name);
        std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
        std::fs::write(
            directory.join(CONTROLLER_INTERRUPTION_MARKER),
            "Motor controller unavailable during recording",
        )
        .map_err(|error| error.to_string())
    }

    // INVARIANT: Preserve controller loss even when recording stops before telemetry arrives.
    pub fn controller_unavailable(&self) {
        self.controller_available.store(false, Ordering::Release);
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let RecordingState::Recording { session, .. } | RecordingState::Stopping(session) =
            &mut *state
        {
            session.controller_interrupted = true;
            if let Err(error) = self.mark_interruption_file(&session.name) {
                tracing::warn!(%error, "Could not mark the recording's controller interruption");
            }
        }
    }

    pub fn controller_ready(&self) {
        self.controller_available.store(true, Ordering::Release);
    }
    #[must_use]
    pub fn active_recording(&self) -> Option<String> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .active_session()
            .map(|session| session.name.clone())
    }
    #[must_use]
    pub fn recording_status(&self) -> RecordingStatus {
        match &*self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
        {
            RecordingState::Idle => RecordingStatus::Idle,
            RecordingState::Recording { session, .. } => RecordingStatus::Recording {
                name: session.name.clone(),
            },
            RecordingState::Stopping(session) => RecordingStatus::Stopping {
                name: session.name.clone(),
            },
            RecordingState::Finished { name } => RecordingStatus::Finished { name: name.clone() },
            RecordingState::Failed { name, detail } => RecordingStatus::Failed {
                name: name.clone(),
                detail: detail.clone(),
            },
        }
    }
    pub fn start_recording(&self) -> Result<String, String> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.shutting_down.load(Ordering::Acquire) {
            return Err("The station is shutting down".into());
        }
        if state.active_session().is_some() {
            return Err("A recording is already active".into());
        }
        let started_at_milliseconds = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| error.to_string())?
            .as_millis();
        let name = format!(
            "{started_at_milliseconds}-{}",
            uuid::Uuid::new_v4().simple()
        );
        let controller_interrupted = !self.controller_available.load(Ordering::Acquire);
        if controller_interrupted {
            self.mark_interruption_file(&name)?;
        }
        let recorder = self.launch_recorder(&name)?;
        let (stop, stopped) = oneshot::channel();
        *state = RecordingState::Recording {
            session: RecordingSession {
                name: name.clone(),
                controller_interrupted,
            },
            stop,
        };
        self.announce(true);
        let manager = self.clone();
        let recording_name = name.clone();
        tokio::spawn(async move {
            let shutdown = async {
                let _ = stopped.await;
            };
            let finalized = recorder
                .wait_or_stop(
                    shutdown,
                    &[
                        (Signal::SIGINT, Duration::from_secs(10)),
                        (Signal::SIGTERM, Duration::from_secs(5)),
                    ],
                    |line| {
                        let _ = writeln!(std::io::stderr().lock(), "Recorder: {line}");
                    },
                )
                .await;
            manager.announce(false);
            let mut state = manager
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let controller_interrupted = state
                .active_session()
                .is_some_and(|session| session.controller_interrupted);
            *state = finalized_recording(recording_name, controller_interrupted, finalized);
            manager.ended.send_replace(());
        });
        Ok(name)
    }

    fn announce(&self, enabled: bool) {
        self.announcements.send_modify(|announcement| {
            announcement.enabled = enabled;
            announcement.revision += 1;
        });
    }

    pub fn stop_recording(&self) {
        self.stop_recording_if(|stop| {
            stop();
            true
        });
    }

    pub fn stop_recording_if(&self, authorize: impl FnOnce(&mut dyn FnMut()) -> bool) -> bool {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        authorize(&mut || state.stop())
    }
    // INVARIANT: Close admission and finish recording/announcement work before dropping the
    // server runtime so pending finalization is not abandoned.
    pub async fn shutdown(&self) {
        self.shutting_down.store(true, Ordering::Release);
        self.stop_recording();
        self.wait_until_inactive().await;
        let revision = self.announcements.borrow().revision;
        let mut announced = self.announced.clone();
        if announced
            .wait_for(|completed| *completed >= revision)
            .await
            .is_err()
        {
            tracing::warn!("Recording announcement worker stopped before shutdown completed");
        }
    }

    pub async fn wait_until_inactive(&self) {
        let mut ended = self.ended.subscribe();
        while self.is_active() {
            if ended.changed().await.is_err() {
                return;
            }
        }
    }
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .active_session()
            .is_some()
    }
    fn launch_recorder(&self, name: &str) -> Result<ChildProcessGroup, String> {
        let configuration = self
            .recording
            .as_ref()
            .ok_or("LiveKit recording is not configured")?;
        let mut station_url =
            reqwest::Url::parse(&self.station_url).map_err(|error| error.to_string());
        station_url = station_url.and_then(|mut url| {
            let scheme = match url.scheme() {
                "http" | "ws" => "http",
                "https" | "wss" => "https",
                scheme => return Err(format!("unsupported station URL scheme: {scheme}")),
            };
            url.set_scheme(scheme)
                .map_err(|()| "invalid station URL".to_owned())?;
            url.set_path("/");
            url.set_query(None);
            url.set_fragment(None);
            Ok(url)
        });
        let station_url = station_url?;
        ChildProcessGroup::spawn(
            Command::new(&configuration.executable)
                .args(["record", "--room", &configuration.room, "--directory"])
                .arg(configuration.directory.join(name))
                .args(["--station-url", station_url.as_str()])
                .args(["--station-pid", &std::process::id().to_string()])
                .stdin(Stdio::null())
                .stdout(Stdio::null()),
        )
        .map_err(|error| error.to_string())
    }
}

// SAFETY: A lease-checked stop must not let a former holder stop the newer holder's recording.
pub fn router(manager: RecordingManager, arms: crate::lease_routes::ServedArms) -> Router {
    Router::new()
        .route("/recording/start", post(start_recording))
        .route("/recording/status", get(recording_status))
        .route("/recording/stop", post(stop_recording))
        .route(
            "/recording/stop-for-lease",
            post(move |State(manager): State<RecordingManager>, Json(request): Json<LeaseToken>| async move {
                let stopped = manager.stop_recording_if(|stop| {
                    arms.get()
                        .and_then(|arms| arms.values().next())
                        .is_some_and(|link| link.with_lease(&request.lease_token, stop))
                });
                if stopped {
                    Ok(Json(()))
                } else {
                    Err(conflict("The lease changed before recording stopped".into()))
                }
            }),
        )
        .with_state(manager)
}

fn conflict(detail: String) -> Refusal {
    crate::lease_routes::refusal(StatusCode::CONFLICT, detail)
}

async fn recording_status(State(manager): State<RecordingManager>) -> Json<RecordingStatus> {
    Json(manager.recording_status())
}

async fn start_recording(State(manager): State<RecordingManager>) -> Result<Json<String>, Refusal> {
    manager.start_recording().map(Json).map_err(conflict)
}

async fn stop_recording(State(manager): State<RecordingManager>) -> Json<()> {
    manager.stop_recording();
    Json(())
}

#[cfg(test)]
mod tests;

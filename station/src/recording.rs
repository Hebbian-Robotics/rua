//! The station's recorder: `hebbian-robot-use-media record` started and
//! stopped on request, independent of power and the lease. Nothing ends a
//! recording implicitly except the station shutting down.

use crate::lease_routes::Refusal;
use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    routing::{get, post},
};
use hebbian_contracts::{CONTROLLER_INTERRUPTION_MARKER, LeaseToken, RecordingStatus};
use nix::{
    sys::signal::{Signal, killpg},
    unistd::Pid,
};
use std::{
    path::PathBuf,
    process::Stdio,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::{Child, Command},
    sync::{mpsc, watch},
    time::Duration,
};
pub struct RecordingConfiguration {
    pub credentials: station_access::Credentials,
    pub room: String,
    pub directory: PathBuf,
    /// Installed `hebbian-robot-use-media` executable. The native media crate is not linked here.
    pub executable: PathBuf,
}
#[derive(Clone)]
pub struct RecordingManager {
    station_url: String,
    recording: Option<Arc<RecordingConfiguration>>,
    announcements: mpsc::UnboundedSender<bool>,
    state: Arc<Mutex<RecordingState>>,
    controller_available: Arc<AtomicBool>,
    /// Ticks each time a recording ends.
    ended: watch::Sender<()>,
}

enum RecordingState {
    Inactive(RecordingStatus),
    Active {
        name: String,
        stop: watch::Sender<bool>,
        stopping: bool,
        controller_interrupted: bool,
    },
}
impl RecordingManager {
    #[must_use]
    pub fn new(configuration: Option<RecordingConfiguration>, station_url: String) -> Self {
        let recording = configuration.map(Arc::new);
        let (announcements, mut receiver) = mpsc::unbounded_channel::<bool>();
        let configuration = recording.clone();
        // Serialize announcements so a delayed start cannot overwrite a later stop.
        // No lifecycle lock is held during the HTTP calls.
        tokio::spawn(async move {
            while let Some(enabled) = receiver.recv().await {
                if let Some(configuration) = &configuration
                    && let Err(error) = configuration
                        .credentials
                        .announce_recording(&configuration.room, enabled)
                        .await
                {
                    eprintln!("Recording announcement failed: {error}");
                }
            }
        });
        Self {
            station_url,
            recording,
            announcements,
            state: Arc::new(Mutex::new(RecordingState::Inactive(RecordingStatus::Idle))),
            controller_available: Arc::new(AtomicBool::new(true)),
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

    /// Preserve controller loss even when recording stops before telemetry arrives.
    pub fn controller_unavailable(&self) {
        self.controller_available.store(false, Ordering::Release);
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let RecordingState::Active {
            name,
            stopping: false,
            controller_interrupted,
            ..
        } = &mut *state
        {
            *controller_interrupted = true;
            if let Err(error) = self.mark_interruption_file(name) {
                eprintln!("Could not mark the recording's controller interruption: {error}");
            }
        }
    }

    pub fn controller_ready(&self) {
        self.controller_available.store(true, Ordering::Release);
    }
    #[must_use]
    pub fn active_recording(&self) -> Option<String> {
        match &*self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
        {
            RecordingState::Active { name, .. } => Some(name.clone()),
            RecordingState::Inactive(_) => None,
        }
    }
    #[must_use]
    pub fn recording_status(&self) -> RecordingStatus {
        match &*self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
        {
            RecordingState::Inactive(status) => status.clone(),
            RecordingState::Active {
                name,
                stopping: false,
                ..
            } => RecordingStatus::Recording { name: name.clone() },
            RecordingState::Active {
                name,
                stopping: true,
                ..
            } => RecordingStatus::Stopping { name: name.clone() },
        }
    }
    /// # Errors
    /// Recording is not configured, or a recording is already active.
    pub fn start_recording(&self) -> Result<String, String> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if matches!(*state, RecordingState::Active { .. }) {
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
        let mut recorder = self.launch_recorder(&name)?;
        let stderr = recorder.stderr.take();
        let (stop, mut stopped) = watch::channel(false);
        *state = RecordingState::Active {
            name: name.clone(),
            stop,
            stopping: false,
            controller_interrupted,
        };
        let _ = self.announcements.send(true);
        let manager = self.clone();
        let recording_name = name.clone();
        tokio::spawn(async move {
            let reader = stderr.map(|pipe| {
                tokio::spawn(async move {
                    let mut lines = BufReader::new(pipe).lines();
                    while let Ok(Some(line)) = lines.next_line().await {
                        eprintln!("Recorder: {line}");
                    }
                })
            });
            let outcome = tokio::select! {
                status = recorder.wait() => RecordingStatus::Failed {
                    name: recording_name.clone(),
                    detail: match status { Ok(status) => format!("Recorder exited unexpectedly: {status}"), Err(error) => error.to_string() },
                },
                _ = stopped.changed() => match interrupt(&mut recorder).await {
                    Ok(status) if status.success() => RecordingStatus::Finished { name: recording_name.clone() },
                    Ok(status) => RecordingStatus::Failed { name: recording_name.clone(), detail: format!("Recorder finalized with {status}; inspect the recording status for details") },
                    Err(error) => RecordingStatus::Failed { name: recording_name.clone(), detail: error.to_string() },
                }
            };
            if let Some(reader) = reader {
                reader.abort();
            }
            let _ = manager.announcements.send(false);
            let mut state = manager
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let outcome = if matches!(
                &*state,
                RecordingState::Active {
                    controller_interrupted: true,
                    ..
                }
            ) && matches!(outcome, RecordingStatus::Finished { .. })
            {
                RecordingStatus::Failed {
                    name: recording_name,
                    detail: "The motor controller was unavailable during the recording".into(),
                }
            } else {
                outcome
            };
            *state = RecordingState::Inactive(outcome);
            manager.ended.send_replace(());
        });
        Ok(name)
    }

    /// Request finalization of the independent recording.
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
        let mut stop = || {
            if let RecordingState::Active { stop, stopping, .. } = &mut *state {
                *stopping = true;
                let _ = stop.send(true);
            }
        };
        authorize(&mut stop)
    }
    /// Stop recording and wait for it to finish before the server drops its runtime.
    pub async fn shutdown(&self) {
        self.stop_recording();
        self.wait_until_inactive().await;
    }

    /// Returns once no recording is active.
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
        matches!(
            *self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            RecordingState::Active { .. }
        )
    }
    fn launch_recorder(&self, name: &str) -> Result<Child, String> {
        let configuration = self
            .recording
            .as_ref()
            .ok_or("LiveKit recording is not configured")?;
        let mut executor_url =
            reqwest::Url::parse(&self.station_url).map_err(|error| error.to_string());
        executor_url = executor_url.and_then(|mut url| {
            let scheme = match url.scheme() {
                "http" | "ws" => "http",
                "https" | "wss" => "https",
                scheme => return Err(format!("unsupported executor URL scheme: {scheme}")),
            };
            url.set_scheme(scheme)
                .map_err(|()| "invalid executor URL".to_owned())?;
            url.set_path("/");
            url.set_query(None);
            url.set_fragment(None);
            Ok(url)
        });
        let executor_url = executor_url?;
        Command::new(&configuration.executable)
            .args(["record", "--room", &configuration.room, "--directory"])
            .arg(configuration.directory.join(name))
            .args(["--executor-url", executor_url.as_str()])
            .args(["--supervisor-pid", &std::process::id().to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .process_group(0)
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| error.to_string())
    }
}

async fn interrupt(child: &mut Child) -> std::io::Result<std::process::ExitStatus> {
    for (signal, grace) in [
        (Signal::SIGINT, 10),
        (Signal::SIGTERM, 5),
        (Signal::SIGKILL, 0),
    ] {
        if let Some(id) = child.id().and_then(|id| i32::try_from(id).ok()) {
            let _ = killpg(Pid::from_raw(id), signal);
        }
        if grace == 0 {
            return child.wait().await;
        }
        if let Ok(status) = tokio::time::timeout(Duration::from_secs(grace), child.wait()).await {
            return status;
        }
    }
    child.wait().await
}

/// `/recording/...`, mounted under `/viewer/api`. `stop-for-lease` stops only
/// while the presented lease is current, so a holder that lost the lease cannot
/// stop someone else's recording.
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

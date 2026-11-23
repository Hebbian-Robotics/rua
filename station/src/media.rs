// CONSTRAINT: Media supervision must never block control, event delivery or the viewer; a
// missing/crashed media child makes cameras unavailable while control continues.

use std::collections::BTreeMap;
use std::io::Write;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use hebbian_contracts::Backoff;
use hebbian_contracts::MediaState;
use hebbian_robot_use_server::child_process_group::{ChildProcessGroup, ProcessExit};
use hebbian_robot_use_server::media_status::MediaStatus;
use nix::sys::signal::Signal;
use station_access::CameraConfiguration;
use tokio::{process::Command, runtime::Handle, sync::oneshot, task::JoinHandle};
const RESTART_DELAY_CAP: Duration = Duration::from_secs(3);
const STABLE_MEDIA_RUN: Duration = Duration::from_secs(3);

const STOP_GRACE: Duration = Duration::from_secs(10);

pub(crate) const SIMULATOR_CAMERA_SOCKET: &str = "scene.camera.sock";

const PROGRAM: &str = "hebbian-robot-use-media";

pub(crate) fn find_media(explicit: Option<PathBuf>) -> PathBuf {
    if let Some(path) = explicit {
        return path;
    }
    let beside = std::env::current_exe()
        .ok()
        .and_then(|executable| executable.parent().map(|directory| directory.join(PROGRAM)))
        .filter(|path| path.is_file());
    beside
        .or_else(|| which::which(PROGRAM).ok())
        .or_else(|| {
            std::env::current_dir()
                .ok()?
                .ancestors()
                .map(|directory| directory.join("target").join("debug").join(PROGRAM))
                .find(|path| path.is_file())
        })
        .unwrap_or_else(|| PathBuf::from(PROGRAM))
}

pub(crate) fn list_camera_devices(explicit: Option<PathBuf>) -> anyhow::Result<()> {
    use anyhow::{Context, ensure};

    let executable = find_media(explicit);
    let status = std::process::Command::new(&executable)
        .arg("camera-devices")
        .status()
        .with_context(|| format!("Cannot run media companion {}", executable.display()))?;
    ensure!(status.success(), "Camera discovery failed: {status}");
    Ok(())
}

pub(crate) struct MediaPlan {
    pub executable: PathBuf,
    pub address: SocketAddr,
    pub room: String,
    pub cameras: BTreeMap<String, String>,
    pub usb_cameras: BTreeMap<String, String>,
    pub simulator_camera_socket: Option<PathBuf>,
}

impl MediaPlan {
    fn serve_arguments(&self, livekit: bool, station_url: &str) -> Vec<String> {
        let mut arguments = vec![
            "serve".to_owned(),
            "--station-pid".to_owned(),
            std::process::id().to_string(),
            "--address".to_owned(),
            self.address.to_string(),
        ];
        if let Some(socket) = &self.simulator_camera_socket {
            arguments.extend(["--camera-socket".to_owned(), socket.display().to_string()]);
        }
        if livekit {
            arguments.extend([
                "--room".to_owned(),
                self.room.clone(),
                "--station-url".to_owned(),
                station_url.to_owned(),
            ]);
        }
        for (name, device) in &self.cameras {
            arguments.extend(["--camera".to_owned(), format!("{name}={device}")]);
        }
        for (name, identity) in &self.usb_cameras {
            arguments.extend(["--usb-camera".to_owned(), format!("{name}={identity}")]);
        }
        arguments
    }
}

pub(crate) struct MediaSupervisor {
    stop: oneshot::Sender<()>,
    task: JoinHandle<()>,
}

impl MediaSupervisor {
    pub(crate) fn start(
        runtime: &Handle,
        plan: &MediaPlan,
        station_url: &str,
        status: &MediaStatus,
        local_cameras_only: bool,
    ) -> Self {
        let livekit = !local_cameras_only
            && match CameraConfiguration::from_environment() {
                Ok(CameraConfiguration::Enabled(_)) => true,
                Ok(CameraConfiguration::Off { reason }) => {
                    tracing::info!(%reason, "LiveKit is off; cameras are local to the media service");
                    false
                }
                Err(error) => {
                    tracing::warn!(%error, "LiveKit is not fully configured; cameras are local");
                    false
                }
            };
        tracing::info!(address = %plan.address, "Media HTTP endpoint");
        Self::spawn(
            runtime,
            plan.executable.clone(),
            plan.serve_arguments(livekit, station_url),
            status.clone(),
        )
    }

    fn spawn(
        runtime: &Handle,
        executable: PathBuf,
        arguments: Vec<String>,
        status: MediaStatus,
    ) -> Self {
        let (stop, shutdown) = oneshot::channel();
        let task = runtime.spawn(async move {
            supervise(&executable, &arguments, shutdown, &status).await;
        });
        Self { stop, task }
    }

    pub(crate) async fn shutdown(self) {
        let _ = self.stop.send(());
        if let Err(error) = self.task.await {
            tracing::error!(%error, "Media supervisor task failed");
        }
    }
}

async fn supervise(
    executable: &Path,
    arguments: &[String],
    mut shutdown: oneshot::Receiver<()>,
    status: &MediaStatus,
) {
    let report = |state: MediaState| {
        status.set(state);
    };
    let mut reported_missing = false;
    let mut backoff = Backoff::new(RESTART_DELAY_CAP);
    while matches!(
        shutdown.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ) {
        match ChildProcessGroup::spawn(Command::new(executable).args(arguments)) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if !reported_missing {
                    tracing::warn!(
                        executable = %executable.display(),
                        "Media executable not found; build it with `CC=clang-21 \
                         CXX=clang++-21 cargo build -p livekit-cameras` or set HEBBIAN_MEDIA"
                    );
                    reported_missing = true;
                }
                report(MediaState::Unavailable {
                    reason: format!("{} not found", executable.display()),
                });
            }
            Err(error) => {
                tracing::warn!(%error, "Cameras unavailable; cannot start media");
                report(MediaState::Unavailable {
                    reason: error.to_string(),
                });
            }
            Ok(child) => {
                let running_since = Instant::now();
                report(MediaState::Running);
                let exit = child
                    .wait_or_stop(
                        async {
                            let _ = (&mut shutdown).await;
                        },
                        &[(Signal::SIGTERM, STOP_GRACE)],
                        |line| {
                            let _ = writeln!(std::io::stderr().lock(), "{line}");
                        },
                    )
                    .await;
                let exit = match exit {
                    Ok(ProcessExit::Requested(_)) => break,
                    Ok(ProcessExit::Unexpected(exit)) => exit,
                    Err(error) => {
                        tracing::error!(%error, "Cannot finish the media process");
                        report(MediaState::Unavailable {
                            reason: format!("Media process cleanup failed: {error}"),
                        });
                        return;
                    }
                };
                let last_exit = exit.to_string();
                tracing::warn!(%last_exit, "Media process exited");
                let (next_backoff, state) =
                    restart_after_media_exit(running_since.elapsed(), last_exit, backoff);
                backoff = next_backoff;
                report(state);
            }
        }
        tokio::select! {
            biased;
            _ = &mut shutdown => break,
            () = tokio::time::sleep(backoff.next_delay()) => {}
        }
    }
    report(MediaState::Unavailable {
        reason: "Media supervisor stopped".to_owned(),
    });
}

#[deny(clippy::disallowed_methods)]
fn restart_after_media_exit(
    run_duration: Duration,
    last_exit: String,
    mut backoff: Backoff,
) -> (Backoff, MediaState) {
    if run_duration >= STABLE_MEDIA_RUN {
        backoff.reset();
    }
    (backoff, MediaState::Restarting { last_exit })
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    #[tokio::test]
    async fn a_program_that_exits_is_restarted_and_reported() {
        let directory = tempfile::tempdir().unwrap();
        let launches = directory.path().join("launches");
        let program = directory.path().join("hebbian-robot-use-media");
        std::fs::write(
            &program,
            format!(
                "#!/bin/sh\necho x >> {launches}\n[ $(wc -l < {launches}) -ge 2 ] && exec sleep 30\nexit 3\n",
                launches = launches.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        let status = MediaStatus::default();
        let supervisor =
            MediaSupervisor::spawn(&Handle::current(), program, Vec::new(), status.clone());
        let deadline = Instant::now() + Duration::from_secs(5);
        while std::fs::read_to_string(&launches)
            .unwrap_or_default()
            .lines()
            .count()
            < 2
        {
            assert!(Instant::now() < deadline, "media did not restart");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(status.report().service, MediaState::Running);
        supervisor.shutdown().await;
        assert!(matches!(
            status.report().service,
            MediaState::Unavailable { .. }
        ));
    }

    #[tokio::test]
    async fn a_missing_program_is_unavailable() {
        let status = MediaStatus::default();
        let supervisor = MediaSupervisor::spawn(
            &Handle::current(),
            PathBuf::from("/no/such/hebbian-robot-use-media"),
            Vec::new(),
            status.clone(),
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        while !matches!(status.report().service, MediaState::Unavailable { reason } if reason.contains("not found"))
        {
            assert!(
                Instant::now() < deadline,
                "missing media executable was not reported"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        supervisor.shutdown().await;
    }
}

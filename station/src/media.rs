//! `hebbian-robot-use-media` beside the station: `serve` (camera snapshots)
//! and, with `LiveKit`, `publish` (the profile's USB cameras into the room).
//!
//! Each runs as a child that is restarted at once after its first exit, then
//! after a delay that doubles up to 3 s while it keeps failing, and
//! stopped with SIGTERM when the station ends. A missing binary or a crash
//! only makes cameras unavailable: nothing here is waited on by control, the event stream
//! or the viewer.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use hebbian_contracts::Backoff;
use hebbian_station::media_status::{MediaState, MediaStatus};
#[cfg(unix)]
use nix::sys::signal::{Signal, kill};
#[cfg(unix)]
use nix::unistd::Pid;
use station_access::CameraConfiguration;
/// The longest wait before starting a program again after it keeps failing.
const RESTART_DELAY_CAP: Duration = Duration::from_secs(3);

const STOP_GRACE: Duration = Duration::from_secs(10);

/// The simulator's camera socket, in the work directory.
pub(crate) const SIMULATOR_CAMERA_SOCKET: &str = "scene.camera.sock";

const PROGRAM: &str = "hebbian-robot-use-media";

/// `hebbian-robot-use-media` beside this executable, else on `PATH`, else the dev build
/// in a `target/debug` above the working directory; the bare name when there
/// is none, which fails to start and reports cameras unavailable.
pub(crate) fn find_media(explicit: Option<PathBuf>) -> PathBuf {
    if let Some(path) = explicit {
        return path;
    }
    let beside = std::env::current_exe()
        .ok()
        .and_then(|executable| executable.parent().map(|directory| directory.join(PROGRAM)));
    let on_path = std::env::var_os("PATH")
        .into_iter()
        .flat_map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>())
        .map(|directory| directory.join(PROGRAM));
    let development = std::env::current_dir()
        .into_iter()
        .flat_map(|directory| {
            directory
                .ancestors()
                .map(Path::to_path_buf)
                .collect::<Vec<_>>()
        })
        .map(|directory| directory.join("target").join("debug").join(PROGRAM));
    beside
        .into_iter()
        .chain(on_path)
        .chain(development)
        .find(|path| path.is_file())
        .unwrap_or_else(|| PathBuf::from(PROGRAM))
}

/// What the media programs are given.
pub(crate) struct MediaPlan {
    pub executable: PathBuf,
    pub address: SocketAddr,
    pub room: String,
    /// The station's HTTP base URL, where `publish` reads telemetry.
    pub station_url: String,
    /// The profile's USB cameras: name to device.
    pub cameras: BTreeMap<String, String>,
    /// Set when a simulator serves camera views.
    pub simulator_camera_socket: Option<PathBuf>,
}

impl MediaPlan {
    fn serve_arguments(&self, livekit: bool) -> Vec<String> {
        let mut arguments = vec![
            "serve".to_owned(),
            "--address".to_owned(),
            self.address.to_string(),
        ];
        if let Some(socket) = &self.simulator_camera_socket {
            arguments.extend(["--camera-socket".to_owned(), socket.display().to_string()]);
        }
        if livekit {
            // `publish` owns the devices; `serve` reads them back from the room.
            arguments.extend(["--room".to_owned(), self.room.clone()]);
        } else {
            for (name, device) in &self.cameras {
                arguments.extend(["--camera".to_owned(), format!("{name}={device}")]);
            }
        }
        arguments
    }

    fn publish_arguments(&self) -> Vec<String> {
        let mut arguments = vec![
            "publish".to_owned(),
            "--room".to_owned(),
            self.room.clone(),
            "--executor-url".to_owned(),
            self.station_url.clone(),
        ];
        for (name, device) in &self.cameras {
            arguments.extend(["--camera".to_owned(), format!("{name}={device}")]);
        }
        arguments
    }
}

/// What a program's supervisor is told.
enum Event {
    Stop,
    Exited(std::io::Result<ExitStatus>),
}

/// Runs the media programs until dropped, which stops them.
pub(crate) struct MediaSupervisor {
    stops: Vec<Sender<Event>>,
    threads: Vec<JoinHandle<()>>,
}

impl MediaSupervisor {
    pub(crate) fn start(plan: &MediaPlan, status: &MediaStatus) -> Self {
        let livekit = match CameraConfiguration::from_environment() {
            Ok(CameraConfiguration::Enabled(_)) => true,
            Ok(CameraConfiguration::Off { reason }) => {
                eprintln!("LiveKit is off ({reason}): cameras are local to the media service");
                false
            }
            Err(error) => {
                eprintln!("LiveKit is not fully configured ({error}): cameras are local");
                false
            }
        };
        let mut programs = vec![("serve", plan.serve_arguments(livekit))];
        if livekit && !plan.cameras.is_empty() {
            programs.push(("publish", plan.publish_arguments()));
        } else {
            status.set("publish", MediaState::NotConfigured);
        }
        let (stops, threads) = programs
            .into_iter()
            .map(|(label, arguments)| {
                let executable = plan.executable.clone();
                let status = status.clone();
                let (sender, events) = std::sync::mpsc::channel();
                let stop = sender.clone();
                let thread = std::thread::spawn(move || {
                    supervise(label, &executable, &arguments, &events, &sender, &status);
                });
                (stop, thread)
            })
            .unzip();
        eprintln!("Media: http://{}", plan.address);
        Self { stops, threads }
    }
}

impl Drop for MediaSupervisor {
    fn drop(&mut self) {
        for stop in &self.stops {
            let _ = stop.send(Event::Stop);
        }
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

/// Run one program, starting it again after it exits (at once the first time,
/// then with a growing delay), until `Event::Stop`. A binary that is not there
/// is reported once and retried the same way. `exits` is the sending side of
/// `events`.
fn supervise(
    label: &str,
    executable: &Path,
    arguments: &[String],
    events: &Receiver<Event>,
    exits: &Sender<Event>,
    status: &MediaStatus,
) {
    let report = |state: MediaState| {
        status.set(label, state);
    };
    let mut reported_missing = false;
    let mut backoff = Backoff::new(RESTART_DELAY_CAP);
    loop {
        let started = Instant::now();
        match Command::new(executable).args(arguments).spawn() {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if !reported_missing {
                    eprintln!(
                        "Cameras unavailable: {} not found (build it with `CC=clang-21 \
                         CXX=clang++-21 cargo build -p livekit-cameras` or set HEBBIAN_MEDIA)",
                        executable.display()
                    );
                    reported_missing = true;
                }
                report(MediaState::Unavailable {
                    reason: format!("{} not found", executable.display()),
                });
            }
            Err(error) => {
                eprintln!("Cameras unavailable: cannot start media {label}: {error}");
                report(MediaState::Unavailable {
                    reason: error.to_string(),
                });
            }
            Ok(child) => {
                report(MediaState::Running);
                let Some(exit) = wait_or_stop(child, events, exits) else {
                    return;
                };
                eprintln!("Media {label} exited ({exit})");
                report(MediaState::Restarting {
                    last_exit: exit.to_string(),
                });
            }
        }
        match events.recv_timeout(backoff.next_delay(started.elapsed())) {
            Err(RecvTimeoutError::Timeout) | Ok(Event::Exited(_)) => {}
            Ok(Event::Stop) | Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

/// The child's exit status, or `None` once a stop ended it (or it cannot be
/// waited for). A thread blocks in `wait` and reports the exit on `exits`.
fn wait_or_stop(
    mut child: Child,
    events: &Receiver<Event>,
    exits: &Sender<Event>,
) -> Option<ExitStatus> {
    let process_id = child.id();
    let exits = exits.clone();
    std::thread::spawn(move || {
        let _ = exits.send(Event::Exited(child.wait()));
    });
    match events.recv() {
        Ok(Event::Exited(Ok(status))) => Some(status),
        Ok(Event::Exited(Err(error))) => {
            eprintln!("Cannot wait for media: {error}");
            None
        }
        Ok(Event::Stop) | Err(_) => {
            terminate(process_id, events);
            None
        }
    }
}

/// SIGTERM (on Unix) lets the program close its files; kill it if it does not
/// exit within the grace period. Returns once the child has been reaped.
fn terminate(process_id: u32, events: &Receiver<Event>) {
    #[cfg(unix)]
    {
        let Ok(pid) = i32::try_from(process_id).map(Pid::from_raw) else {
            return;
        };
        let _ = kill(pid, Signal::SIGTERM);
        if matches!(
            events.recv_timeout(STOP_GRACE),
            Err(RecvTimeoutError::Timeout)
        ) {
            let _ = kill(pid, Signal::SIGKILL);
            let _ = events.recv();
        }
    }
    #[cfg(not(unix))]
    let _ = (process_id, events);
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    #[test]
    fn a_program_that_exits_is_restarted_and_reported() {
        let directory = tempfile::tempdir().unwrap();
        let launches = directory.path().join("launches");
        let program = directory.path().join("hebbian-robot-use-media");
        // Exits with an error the first time, then keeps running.
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
        let (stop, events) = std::sync::mpsc::channel();
        let thread = {
            let (status, exits) = (status.clone(), stop.clone());
            std::thread::spawn(move || {
                supervise("serve", &program, &[], &events, &exits, &status);
            })
        };
        status.wait_until("serve", |state| {
            matches!(state, MediaState::Restarting { .. })
        });
        status.wait_until("serve", |state| *state == MediaState::Running);
        stop.send(Event::Stop).unwrap();
        thread.join().unwrap();
    }

    #[test]
    fn a_missing_program_is_unavailable() {
        let status = MediaStatus::default();
        let (stop, events) = std::sync::mpsc::channel();
        let thread = {
            let (status, exits) = (status.clone(), stop.clone());
            std::thread::spawn(move || {
                supervise(
                    "serve",
                    Path::new("/no/such/hebbian-robot-use-media"),
                    &[],
                    &events,
                    &exits,
                    &status,
                );
            })
        };
        status.wait_until(
            "serve",
            |state| matches!(state, MediaState::Unavailable { reason } if reason.contains("not found")),
        );
        stop.send(Event::Stop).unwrap();
        thread.join().unwrap();
    }
}

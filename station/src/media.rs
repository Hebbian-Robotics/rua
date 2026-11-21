// CONSTRAINT: Media supervision must never block control, event delivery or the viewer; a
// missing/crashed media child makes cameras unavailable while control continues.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader};
use std::net::SocketAddr;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use hebbian_contracts::Backoff;
use hebbian_contracts::MediaState;
use hebbian_robot_use_server::media_status::MediaStatus;
#[cfg(unix)]
use nix::sys::signal::{Signal, killpg};
#[cfg(unix)]
use nix::unistd::Pid;
use station_access::CameraConfiguration;
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

pub(crate) struct MediaPlan {
    pub executable: PathBuf,
    pub address: SocketAddr,
    pub room: String,
    pub cameras: BTreeMap<String, String>,
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
            // WHY: `publish` owns the devices; `serve` reads them back from the room.
            arguments.extend(["--room".to_owned(), self.room.clone()]);
            // WHY: A camera that fails to open never publishes; name it so it is listed as unavailable.
            for name in self.cameras.keys() {
                arguments.extend(["--expected-camera".to_owned(), name.clone()]);
            }
        } else {
            for (name, device) in &self.cameras {
                arguments.extend(["--camera".to_owned(), format!("{name}={device}")]);
            }
        }
        arguments
    }

    fn publish_arguments(&self, station_url: &str) -> Vec<String> {
        let mut arguments = vec![
            "publish".to_owned(),
            "--room".to_owned(),
            self.room.clone(),
            "--station-url".to_owned(),
            station_url.to_owned(),
        ];
        for (name, device) in &self.cameras {
            arguments.extend(["--camera".to_owned(), format!("{name}={device}")]);
        }
        arguments
    }
}

enum Event {
    Stop,
    Exited(std::io::Result<ExitStatus>),
}

pub(crate) struct MediaSupervisor {
    stops: Vec<Sender<Event>>,
    threads: Vec<JoinHandle<()>>,
}

impl MediaSupervisor {
    pub(crate) fn start(plan: &MediaPlan, station_url: &str, status: &MediaStatus) -> Self {
        let livekit = match CameraConfiguration::from_environment() {
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
        let mut programs = vec![("serve", plan.serve_arguments(livekit))];
        if livekit && !plan.cameras.is_empty() {
            programs.push(("publish", plan.publish_arguments(station_url)));
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
        tracing::info!(address = %plan.address, "Media HTTP endpoint");
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
        match Command::new(executable)
            .args(arguments)
            .process_group(0)
            .stderr(Stdio::piped())
            .spawn()
        {
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
                tracing::warn!(%label, %error, "Cameras unavailable; cannot start media");
                report(MediaState::Unavailable {
                    reason: error.to_string(),
                });
            }
            Ok(mut child) => {
                let running_since = Instant::now();
                if let Some(stderr) = child.stderr.take() {
                    let status = status.clone();
                    std::thread::spawn(move || relay_output(stderr, &status));
                }
                report(MediaState::Running);
                let Some(exit) = wait_or_stop(child, events, exits) else {
                    break;
                };
                // WHY: Spawn alone does not establish that a media process can stay up.
                if running_since.elapsed() >= STABLE_MEDIA_RUN {
                    backoff.reset();
                }
                tracing::warn!(%label, %exit, "Media process exited");
                report(MediaState::Restarting {
                    last_exit: exit.to_string(),
                });
            }
        }
        match events.recv_timeout(backoff.next_delay()) {
            Err(RecvTimeoutError::Timeout) | Ok(Event::Exited(_)) => {}
            Ok(Event::Stop) | Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    report(MediaState::Unavailable {
        reason: "Media supervisor stopped".to_owned(),
    });
}

fn relay_output(output: impl std::io::Read, status: &MediaStatus) {
    for line in BufReader::new(output).lines().map_while(Result::ok) {
        eprintln!("{line}");
        if let Some((camera, detail)) = parse_camera_problem(&line) {
            status.record_camera_problem(camera, detail);
        }
    }
}

// CONSTRAINT: Capture failures use `Camera <name>: <message>`; reopening lines only announce
// a retry and must not replace the original failure detail.
fn parse_camera_problem(line: &str) -> Option<(&str, &str)> {
    let (camera, detail) = line.strip_prefix("Camera ")?.split_once(": ")?;
    let valid_name = (1..=40).contains(&camera.len())
        && camera
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_');
    let detail = detail.trim();
    (valid_name && !detail.is_empty() && !detail.starts_with("reopening "))
        .then_some((camera, detail))
}

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
        Ok(Event::Exited(Ok(status))) => {
            signal_group(process_id, Signal::SIGKILL);
            Some(status)
        }
        Ok(Event::Exited(Err(error))) => {
            tracing::warn!(%error, "Cannot wait for media");
            signal_group(process_id, Signal::SIGKILL);
            None
        }
        Ok(Event::Stop) | Err(_) => {
            terminate(process_id, events);
            None
        }
    }
}

fn signal_group(process_id: u32, signal: Signal) {
    if let Ok(pid) = i32::try_from(process_id) {
        let _ = killpg(Pid::from_raw(pid), signal);
    }
}

// WHY: The leader can exit before its capture workers. Reap the leader and
// check for remaining group members even when it exits within the grace period.
fn terminate(process_id: u32, events: &Receiver<Event>) {
    signal_group(process_id, Signal::SIGTERM);
    let exit = events.recv_timeout(STOP_GRACE);
    signal_group(process_id, Signal::SIGKILL);
    if matches!(exit, Err(RecvTimeoutError::Timeout) | Ok(Event::Stop)) {
        while matches!(events.recv(), Ok(Event::Stop)) {}
    }
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
    fn camera_problems_are_read_from_the_capture_failure_lines() {
        assert_eq!(
            parse_camera_problem(
                "Camera wrist: camera backend: Could not open device 0x1: Device is null"
            ),
            Some((
                "wrist",
                "camera backend: Could not open device 0x1: Device is null"
            ))
        );
        for ignored in [
            "Camera wrist: reopening 0x1 in 3s",
            "Camera Wrist: broken",
            "Camera : broken",
            "Camera wrist:",
            "Camera initialization unavailable: no backend",
            "Media serve exited (exit status: 1)",
        ] {
            assert_eq!(parse_camera_problem(ignored), None, "{ignored}");
        }
    }

    #[test]
    fn a_program_that_cannot_open_a_camera_has_the_problem_reported() {
        let directory = tempfile::tempdir().unwrap();
        let program = directory.path().join("hebbian-robot-use-media");
        std::fs::write(
            &program,
            "#!/bin/sh\nwhile true; do\n\
             echo 'Camera wrist: camera backend: Could not open device 0x1: Device is null' >&2\n\
             echo 'Camera wrist: reopening 0x1 in 3s' >&2\nsleep 0.05\ndone\n",
        )
        .unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        let status = MediaStatus::default();
        let (stop, events) = std::sync::mpsc::channel();
        let thread = {
            let (status, exits) = (status.clone(), stop.clone());
            std::thread::spawn(move || {
                supervise("publish", &program, &[], &events, &exits, &status);
            })
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        let problem = loop {
            if let Some(problem) = status.report().cameras.get("wrist") {
                break problem.clone();
            }
            assert!(Instant::now() < deadline, "no problem was reported");
            std::thread::sleep(Duration::from_millis(20));
        };
        assert_eq!(
            problem.detail,
            "camera backend: Could not open device 0x1: Device is null"
        );
        assert!(problem.last_seen_unix_seconds > 0.0);
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

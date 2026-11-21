use std::collections::VecDeque;
use std::io::{BufRead, BufReader};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{Context, ensure};
use hebbian_robot_use_core::controller_transport::ControllerInstanceId;
use uuid::Uuid;

const REMEMBERED_REPORTS: usize = 128;

pub enum PlantSelection {
    Servos,
    Simulator { socket_directory: PathBuf },
    NoPhysicsStandIn { lag_seconds: f64 },
}

pub struct ControllerLaunch {
    pub executable: PathBuf,
    pub arms: Vec<(String, PathBuf)>,
    pub work_directory: PathBuf,
    pub plant: PlantSelection,
}

pub struct ControllerProcess {
    child: Child,
    listener: UnixListener,
    socket_path: PathBuf,
    instance: Uuid,
    reports: Arc<Mutex<ControllerReports>>,
    report_pipe_closed: Arc<AtomicBool>,
}

#[derive(Default)]
struct ControllerReports {
    lines: VecDeque<String>,
    changed: bool,
}

impl ControllerProcess {
    pub fn start(configuration: &ControllerLaunch) -> anyhow::Result<Self> {
        ensure!(!configuration.arms.is_empty(), "A controller needs an arm");
        let instance = Uuid::new_v4();
        let socket_path = configuration
            .work_directory
            .join(format!("ctl-{}.sock", instance.simple()));
        let listener = UnixListener::bind(&socket_path)
            .with_context(|| format!("Cannot listen on {}", socket_path.display()))?;
        listener.set_nonblocking(true)?;
        let mut command = Command::new(&configuration.executable);
        command
            .arg("controller")
            .arg("--station-socket")
            .arg(&socket_path)
            .arg("--instance")
            .arg(instance.to_string())
            .stdin(Stdio::null())
            .stderr(Stdio::piped());
        for (arm, config) in &configuration.arms {
            command
                .arg("--arm")
                .arg(format!("{arm}={}", config.display()));
        }
        match &configuration.plant {
            PlantSelection::Servos => command.arg("--hardware"),
            PlantSelection::Simulator { socket_directory } => {
                command.arg("--socket-directory").arg(socket_directory)
            }
            PlantSelection::NoPhysicsStandIn { lag_seconds } => command
                .arg("--plant-lag-seconds")
                .arg(lag_seconds.to_string()),
        };
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                let _ = std::fs::remove_file(&socket_path);
                return Err(error).context("Cannot start the motor controller");
            }
        };
        let stderr = child
            .stderr
            .take()
            .context("The controller has no report pipe")?;
        let reports = Arc::new(Mutex::new(ControllerReports::default()));
        let report_pipe_closed = Arc::new(AtomicBool::new(false));
        let received_reports = Arc::clone(&reports);
        let pipe_closed = Arc::clone(&report_pipe_closed);
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stderr);
            loop {
                let mut line = String::new();
                match reader.read_line(&mut line) {
                    Ok(0) => {
                        pipe_closed.store(true, Ordering::Release);
                        break;
                    }
                    Ok(_) => {
                        eprint!("{line}");
                        let mut reports = received_reports
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        if reports.lines.len() == REMEMBERED_REPORTS {
                            reports.lines.pop_front();
                        }
                        line.truncate(line.trim_end().len());
                        reports.lines.push_back(line);
                        reports.changed = true;
                    }
                    Err(error) => {
                        eprintln!("Controller report pipe failed: {error}");
                        break;
                    }
                }
            }
        });
        Ok(Self {
            child,
            listener,
            socket_path,
            instance,
            reports,
            report_pipe_closed,
        })
    }

    #[must_use]
    pub fn instance(&self) -> ControllerInstanceId {
        ControllerInstanceId(*self.instance.as_bytes())
    }

    #[must_use]
    pub fn process_id(&self) -> u32 {
        self.child.id()
    }

    pub fn try_accept(&self) -> anyhow::Result<Option<UnixStream>> {
        match self.listener.accept() {
            Ok((socket, _)) => {
                // CONSTRAINT: Accepted sockets inherit the listener's nonblocking mode on
                // some platforms; controller transport performs blocking reads.
                socket.set_nonblocking(false)?;
                Ok(Some(socket))
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
            Err(error) => Err(error).context("Cannot accept the controller connection"),
        }
    }

    pub fn try_wait(&mut self) -> anyhow::Result<Option<ExitStatus>> {
        self.child
            .try_wait()
            .context("Cannot inspect the controller process")
    }

    // SAFETY: Stopping the controller does not stop its independent guard; the guard remains
    // responsible for the arm until release is verified.
    pub fn stop_controller(&mut self) -> anyhow::Result<()> {
        if !matches!(self.child.try_wait(), Ok(Some(_))) {
            self.child
                .kill()
                .context("Cannot stop the motor controller")?;
        }
        Ok(())
    }

    // SAFETY: A closed stderr pipe confirms controller/guard process release, not motor torque.
    #[must_use]
    pub fn report_pipe_closed(&self) -> bool {
        self.report_pipe_closed.load(Ordering::Acquire)
    }

    #[must_use]
    pub(crate) fn take_updated_reports(&self) -> Option<Vec<String>> {
        let mut reports = self
            .reports
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        std::mem::take(&mut reports.changed).then(|| reports.lines.iter().cloned().collect())
    }

    #[must_use]
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }
}

impl Drop for ControllerProcess {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

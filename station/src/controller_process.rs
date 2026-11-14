//! Launch and observe a motor-controller child and its independent guards.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{Context, ensure};
use hebbian_robot_use_server::controller_transport::ControllerInstanceId;
use uuid::Uuid;

const REMEMBERED_REPORTS: usize = 128;

/// The exact configuration files and plants the child will own.
pub struct ControllerLaunch {
    pub executable: PathBuf,
    pub arms: Vec<(String, PathBuf)>,
    pub work_directory: PathBuf,
    pub simulator_socket_directory: Option<PathBuf>,
    pub plant_lag_seconds: f64,
    pub hardware: bool,
}

/// A child and the guard reports inherited through its stderr pipe.
pub struct ControllerProcess {
    child: Child,
    listener: UnixListener,
    socket_path: PathBuf,
    instance: Uuid,
    reports: Arc<Mutex<VecDeque<String>>>,
    report_pipe_closed: Arc<AtomicBool>,
}

impl ControllerProcess {
    /// Start a fresh controller with a unique instance and private socket path.
    ///
    /// # Errors
    /// The socket cannot be created or the child cannot start.
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
            .arg("--plant-lag-seconds")
            .arg(configuration.plant_lag_seconds.to_string())
            .stdin(Stdio::null())
            .stderr(Stdio::piped());
        for (arm, config) in &configuration.arms {
            command
                .arg("--arm")
                .arg(format!("{arm}={}", config.display()));
        }
        if configuration.hardware {
            command.arg("--hardware");
        }
        if let Some(directory) = &configuration.simulator_socket_directory {
            command.arg("--socket-directory").arg(directory);
        }
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
        let reports = Arc::new(Mutex::new(VecDeque::new()));
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
                        if reports.len() == REMEMBERED_REPORTS {
                            reports.pop_front();
                        }
                        reports.push_back(line.trim_end().to_owned());
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

    /// Accept a controller connection without blocking the station's supervisor.
    ///
    /// # Errors
    /// The listening socket fails.
    pub fn try_accept(&self) -> anyhow::Result<Option<UnixStream>> {
        match self.listener.accept() {
            Ok((socket, _)) => {
                // Accepted sockets inherit the listener's nonblocking mode on
                // some platforms; controller transport performs blocking reads.
                socket.set_nonblocking(false)?;
                Ok(Some(socket))
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
            Err(error) => Err(error).context("Cannot accept the controller connection"),
        }
    }

    /// # Errors
    /// The operating system cannot inspect the child.
    pub fn try_wait(&mut self) -> anyhow::Result<Option<ExitStatus>> {
        self.child
            .try_wait()
            .context("Cannot inspect the controller process")
    }

    /// Stop only the controller. Any independent guard remains responsible for the arm.
    ///
    /// # Errors
    /// The operating system cannot stop the child.
    pub fn stop_controller(&mut self) -> anyhow::Result<()> {
        if self.try_wait()?.is_none() {
            self.child
                .kill()
                .context("Cannot stop the motor controller")?;
        }
        Ok(())
    }

    /// True only after the controller and any guard inheriting its stderr have
    /// closed that pipe. This confirms process release, not motor torque.
    #[must_use]
    pub fn report_pipe_closed(&self) -> bool {
        self.report_pipe_closed.load(Ordering::Acquire)
    }

    #[must_use]
    pub fn reports(&self) -> Vec<String> {
        self.reports
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .cloned()
            .collect()
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

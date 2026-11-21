use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read};
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
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
}

#[derive(Default, PartialEq, Eq)]
enum ReportPipe {
    #[default]
    Reading,
    Eof,
    Failed,
}

#[derive(Default)]
struct ControllerReports {
    pipe: ReportPipe,
    lines: VecDeque<String>,
    changed: bool,
}

impl ControllerReports {
    fn record(&mut self, line: String) {
        if self.lines.len() == REMEMBERED_REPORTS {
            self.lines.pop_front();
        }
        self.lines.push_back(line);
        self.changed = true;
    }
}

fn read_reports(stderr: impl Read, reports: &Mutex<ControllerReports>) {
    let mut reader = BufReader::new(stderr);
    let mut bytes = Vec::new();
    let (pipe, failure) = loop {
        match reader.read_until(b'\n', &mut bytes) {
            Ok(0) => break (ReportPipe::Eof, None),
            Ok(_) => {
                let line = String::from_utf8_lossy(&bytes);
                eprint!("{line}");
                reports
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .record(line.trim_end().to_owned());
                bytes.clear();
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => {
                break (
                    ReportPipe::Failed,
                    Some(format!("Controller report pipe failed: {error}")),
                );
            }
        }
    };
    let mut reports = reports
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    reports.pipe = pipe;
    if let Some(failure) = failure {
        tracing::warn!("{failure}");
        reports.record(failure);
    }
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
            .process_group(0)
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
        let received_reports = Arc::clone(&reports);
        std::thread::spawn(move || read_reports(stderr, &received_reports));
        Ok(Self {
            child,
            listener,
            socket_path,
            instance,
            reports,
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
        self.reports
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pipe
            == ReportPipe::Eof
    }

    // SAFETY: A failed reader proves nothing about the inherited writer.
    // Only a gone process group permits reconnect in that case.
    pub(crate) fn guard_released(&mut self) -> bool {
        if !matches!(self.try_wait(), Ok(Some(_))) {
            return false;
        }
        let reports = self
            .reports
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match reports.pipe {
            ReportPipe::Reading => false,
            ReportPipe::Eof => true,
            ReportPipe::Failed => i32::try_from(self.child.id()).is_ok_and(|id| {
                nix::sys::signal::killpg(nix::unistd::Pid::from_raw(id), None)
                    == Err(nix::errno::Errno::ESRCH)
            }),
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    struct FailedReportPipe;

    impl Read for FailedReportPipe {
        fn read(&mut self, _destination: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("report pipe I/O error"))
        }
    }

    #[test]
    fn report_pipe_failure_is_visible_and_distinct_from_guard_eof() {
        let reports = Mutex::new(ControllerReports::default());
        read_reports(FailedReportPipe, &reports);
        let reports = reports
            .into_inner()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert!(reports.pipe == ReportPipe::Failed);
        assert!(reports.changed && reports.lines[0].contains("report pipe I/O error"));
    }
}

use std::{
    future::Future,
    io,
    process::{ExitStatus, Stdio},
    time::Duration,
};

use nix::{
    errno::Errno,
    sys::signal::{Signal, killpg},
    unistd::Pid,
};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::{Child, ChildStderr, Command},
    time::timeout,
};

pub enum ChildExit {
    Requested(ExitStatus),
    Unexpected(ExitStatus),
}

pub struct ManagedChild {
    leader: Child,
    group: Option<Pid>,
}

impl ManagedChild {
    pub fn spawn(command: &mut Command) -> io::Result<Self> {
        let leader = command
            .process_group(0)
            .kill_on_drop(true)
            .stderr(Stdio::piped())
            .spawn()?;
        let group = leader
            .id()
            .and_then(|id| i32::try_from(id).ok())
            .filter(|&id| id > 0)
            .map(Pid::from_raw)
            .ok_or_else(|| io::Error::other("Cannot identify the companion process group"))?;
        Ok(Self {
            leader,
            group: Some(group),
        })
    }

    pub async fn finish(
        mut self,
        shutdown: impl Future<Output = ()>,
        graceful_signals: &[(Signal, Duration)],
        report: impl FnMut(&str),
    ) -> io::Result<ChildExit> {
        let stderr = self.leader.stderr.take();
        let relay = relay_stderr(stderr, report);
        let exit = self.wait_for_exit(shutdown, graceful_signals);
        tokio::pin!(relay, exit);
        tokio::select! {
            outcome = &mut exit => {
                // WHY: Keep queued diagnostics without waiting indefinitely on a stray pipe holder.
                let _ = timeout(Duration::from_millis(200), relay).await;
                outcome
            }
            () = &mut relay => exit.await,
        }
    }

    async fn wait_for_exit(
        &mut self,
        shutdown: impl Future<Output = ()>,
        graceful_signals: &[(Signal, Duration)],
    ) -> io::Result<ChildExit> {
        let outcome = tokio::select! {
            biased;
            status = self.leader.wait() => status.map(ChildExit::Unexpected),
            () = shutdown => self.stop(graceful_signals).await.map(ChildExit::Requested),
        };
        // WHY: The leader may already be reaped while its workers still hold files or devices.
        self.kill_group()?;
        outcome
    }

    async fn stop(&mut self, graceful_signals: &[(Signal, Duration)]) -> io::Result<ExitStatus> {
        for &(signal, grace) in graceful_signals {
            self.signal_group(signal)?;
            if let Ok(status) = timeout(grace, self.leader.wait()).await {
                return status;
            }
        }
        self.kill_group()?;
        self.leader.wait().await
    }

    fn kill_group(&mut self) -> io::Result<()> {
        self.signal_group(Signal::SIGKILL)?;
        self.group = None;
        Ok(())
    }

    fn signal_group(&self, signal: Signal) -> io::Result<()> {
        let Some(group) = self.group else {
            return Ok(());
        };
        match killpg(group, signal) {
            // WHY: An empty group already satisfies shutdown.
            Ok(()) | Err(Errno::ESRCH) => Ok(()),
            Err(error) => Err(io::Error::other(format!(
                "Cannot send {signal} to companion process group {group}: {error}"
            ))),
        }
    }
}

impl Drop for ManagedChild {
    fn drop(&mut self) {
        if let Err(error) = self.kill_group() {
            tracing::warn!(%error, "Companion process cleanup failed");
        }
    }
}

async fn relay_stderr(stderr: Option<ChildStderr>, mut report: impl FnMut(&str)) {
    let Some(stderr) = stderr else {
        return;
    };
    let mut reader = BufReader::new(stderr);
    let mut bytes = Vec::new();
    loop {
        match reader.read_until(b'\n', &mut bytes).await {
            Ok(0) => return,
            Ok(_) => {
                report(String::from_utf8_lossy(&bytes).trim_end_matches(['\r', '\n']));
                bytes.clear();
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => {
                tracing::warn!(%error, "Cannot read companion stderr");
                return;
            }
        }
    }
}

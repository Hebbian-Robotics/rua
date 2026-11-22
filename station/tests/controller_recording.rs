use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

use hebbian_contracts::{CONTROLLER_INTERRUPTION_MARKER, RecordingStatus};
use hebbian_robot_use_server::recording::{RecordingConfiguration, RecordingManager};
use station_access::Credentials;

fn recording_manager(
    root: &std::path::Path,
    executable: std::path::PathBuf,
) -> Result<RecordingManager, Box<dyn std::error::Error>> {
    let credentials = Credentials::from_values([
        Some("ws://127.0.0.1:1".into()),
        Some("key".into()),
        Some("secret".into()),
    ])?;
    Ok(RecordingManager::new(
        Some(RecordingConfiguration {
            credentials,
            room: "test".into(),
            directory: root.join("recordings"),
            executable,
        }),
        "http://127.0.0.1:1".into(),
    ))
}

#[tokio::test]
async fn recording_started_offline_stays_incomplete_after_controller_recovers()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let executable = directory.path().join("recorder");
    let ready = directory.path().join("recorder-ready");
    std::fs::write(
        &executable,
        format!(
            "#!/bin/sh\ntrap 'exit 0' INT TERM\nprintf ready > '{}'\nwhile :; do sleep 1; done\n",
            ready.display()
        ),
    )?;
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755))?;
    let manager = recording_manager(directory.path(), executable)?;
    manager.controller_unavailable();
    let name = manager.start_recording()?;
    assert!(
        directory
            .path()
            .join("recordings")
            .join(&name)
            .join(CONTROLLER_INTERRUPTION_MARKER)
            .exists()
    );
    manager.controller_ready();
    tokio::time::timeout(Duration::from_secs(5), async {
        while !ready.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await?;
    manager.stop_recording();
    tokio::time::timeout(Duration::from_secs(5), manager.wait_until_inactive()).await?;
    assert!(
        matches!(manager.recording_status(), RecordingStatus::Failed { detail, .. } if detail.contains("controller"))
    );
    std::fs::remove_file(&ready)?;
    let active_name = manager.start_recording()?;
    tokio::time::timeout(Duration::from_secs(5), async {
        while !ready.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await?;
    manager.stop_recording();
    manager.stop_recording();
    assert!(
        matches!(manager.recording_status(), RecordingStatus::Stopping { name } if name == active_name)
    );
    manager.controller_unavailable();
    assert!(
        directory
            .path()
            .join("recordings")
            .join(&active_name)
            .join(CONTROLLER_INTERRUPTION_MARKER)
            .exists()
    );
    tokio::time::timeout(Duration::from_secs(5), manager.wait_until_inactive()).await?;
    assert!(
        matches!(manager.recording_status(), RecordingStatus::Failed { detail, .. } if detail.contains("controller"))
    );
    Ok(())
}

#[tokio::test]
async fn recorder_workers_are_stopped_when_the_leader_exits()
-> Result<(), Box<dyn std::error::Error>> {
    for shutdown in [RecorderShutdown::Requested, RecorderShutdown::Unexpected] {
        exercise_recorder_cleanup(shutdown).await?;
    }
    Ok(())
}

#[derive(Clone, Copy, Debug)]
enum RecorderShutdown {
    Requested,
    Unexpected,
}

struct RecorderGroupCleanup(Option<nix::unistd::Pid>);

impl Drop for RecorderGroupCleanup {
    fn drop(&mut self) {
        if let Some(group) = self.0.take() {
            let _ = nix::sys::signal::killpg(group, nix::sys::signal::Signal::SIGKILL);
        }
    }
}

async fn exercise_recorder_cleanup(
    shutdown: RecorderShutdown,
) -> Result<(), Box<dyn std::error::Error>> {
    use tokio::io::AsyncReadExt;

    let directory = tempfile::tempdir()?;
    let executable = directory.path().join("recorder");
    let worker_pipe = directory.path().join("worker-pipe");
    let leader_pid = directory.path().join("leader-pid");
    let worker_ready = directory.path().join("worker-ready");
    let quit = directory.path().join("quit");
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&worker_pipe)
            .status()?
            .success()
    );
    let mut worker_output =
        tokio::net::unix::pipe::OpenOptions::new().open_receiver(&worker_pipe)?;
    std::fs::write(
        &executable,
        format!(
            "#!/bin/sh\ntrap 'exit 0' INT TERM\nprintf '%s' \"$$\" > '{}'\n(\n  trap '' INT TERM\n  exec 3>'{}'\n  printf x >&3\n  touch '{}'\n  exec sleep 60\n) &\nwhile [ ! -e '{}' ]; do sleep 0.02; done\nexit 17\n",
            leader_pid.display(),
            worker_pipe.display(),
            worker_ready.display(),
            quit.display(),
        ),
    )?;
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755))?;
    let manager = recording_manager(directory.path(), executable)?;

    let name = manager.start_recording()?;
    let deadline = Duration::from_secs(5);
    tokio::time::timeout(deadline, async {
        while !worker_ready.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await?;
    let mut cleanup = RecorderGroupCleanup(Some(nix::unistd::Pid::from_raw(
        std::fs::read_to_string(leader_pid)?.parse()?,
    )));
    assert_eq!(
        tokio::time::timeout(deadline, worker_output.read_u8()).await??,
        b'x'
    );

    match shutdown {
        RecorderShutdown::Requested => manager.stop_recording(),
        RecorderShutdown::Unexpected => std::fs::write(quit, [])?,
    }
    tokio::time::timeout(deadline, manager.wait_until_inactive()).await?;
    match shutdown {
        RecorderShutdown::Requested => assert!(
            matches!(manager.recording_status(), RecordingStatus::Finished { name: finished } if name == finished)
        ),
        RecorderShutdown::Unexpected => assert!(
            matches!(manager.recording_status(), RecordingStatus::Failed { name: failed, detail } if name == failed && detail.contains("exited unexpectedly"))
        ),
    }
    let mut remaining = [0];
    let end =
        tokio::time::timeout(Duration::from_secs(1), worker_output.read(&mut remaining)).await;
    assert!(
        matches!(end, Ok(Ok(0))),
        "{shutdown:?}: recorder worker kept its pipe open: {end:?}"
    );
    cleanup.0 = None;
    Ok(())
}

//! Recording lifecycle when the motor controller is unavailable.

use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

use hebbian_contracts::{CONTROLLER_INTERRUPTION_MARKER, RecordingStatus};
use hebbian_station::recording::{RecordingConfiguration, RecordingManager};
use station_access::Credentials;

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
    let credentials = Credentials::from_values([
        Some("ws://127.0.0.1:1".into()),
        Some("key".into()),
        Some("secret".into()),
    ])?;
    let manager = RecordingManager::new(
        Some(RecordingConfiguration {
            credentials,
            room: "test".into(),
            directory: directory.path().join("recordings"),
            executable,
        }),
        "http://127.0.0.1:1".into(),
    );
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
    manager.controller_unavailable();
    manager.stop_recording();
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

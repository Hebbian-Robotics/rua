use std::time::Duration;

const SUPERVISOR_POLL: Duration = Duration::from_millis(500);

/// Resolve when the supervising process has exited. Standalone media commands
/// have no supervisor and continue until their usual shutdown signal.
pub async fn exited(supervisor_pid: Option<u32>) {
    let Some(supervisor_pid) = supervisor_pid else {
        return std::future::pending().await;
    };
    while std::os::unix::process::parent_id() == supervisor_pid {
        tokio::time::sleep(SUPERVISOR_POLL).await;
    }
}

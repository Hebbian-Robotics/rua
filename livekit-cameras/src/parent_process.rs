use std::time::Duration;

const PARENT_POLL_INTERVAL: Duration = Duration::from_millis(500);

// WHY: After the station exits, the OS reparents its media child.
pub async fn wait_for_station_exit(station_pid: Option<u32>) {
    let Some(station_pid) = station_pid else {
        return std::future::pending().await;
    };
    while std::os::unix::process::parent_id() == station_pid {
        tokio::time::sleep(PARENT_POLL_INTERVAL).await;
    }
}

pub(crate) mod archive;
mod error;
#[cfg(test)]
mod tests;
mod video_output;

use archive::RecordingArchive;
use clap::Parser;
pub use error::RecorderError;
use hebbian_contracts::{CONTROLLER_INTERRUPTION_MARKER, unix_nanoseconds};
use livekit_camera_recorder::{VideoOutput, hardware_encoder_available, record_room};
use serde_json::json;
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio_util::sync::CancellationToken;

#[derive(Parser)]
#[command(about = "Record every camera in a LiveKit room, with station telemetry, to one MCAP")]
pub struct Arguments {
    #[arg(long)]
    room: String,
    #[arg(long)]
    directory: PathBuf,
    #[arg(long, default_value = "http://127.0.0.1:8792")]
    pub(crate) station_url: String,
    /// The process that started this recorder. When it is no longer this
    /// process's parent, the recorder finalizes the file as incomplete and exits.
    #[arg(long)]
    pub(crate) supervisor_pid: Option<u32>,
}

/// How often a recorder checks that its supervisor still exists.
const SUPERVISOR_POLL: Duration = Duration::from_millis(500);

/// A guard is its supervisor's child, so it is reparented the moment the supervisor exits.
fn supervisor_alive(supervisor_pid: u32) -> bool {
    nix::unistd::getppid().as_raw().unsigned_abs() == supervisor_pid
}

/// Cancel `stopped` and set `lost` once `supervisor_pid` has gone.
async fn watch_supervisor(supervisor_pid: u32, lost: Arc<AtomicBool>, stopped: CancellationToken) {
    loop {
        tokio::select! {
            () = stopped.cancelled() => return,
            () = tokio::time::sleep(SUPERVISOR_POLL) => {}
        }
        if !supervisor_alive(supervisor_pid) {
            eprintln!(
                "Supervising station (pid {supervisor_pid}) is gone; finalizing the recording"
            );
            lost.store(true, Ordering::SeqCst);
            stopped.cancel();
            return;
        }
    }
}

/// Record until Ctrl-C or SIGTERM, then finish the encoders and MCAP index.
///
/// # Errors
/// Returns credential, room, file, encoding, or MCAP errors. A single camera's
/// failure is logged to `/station/recording_errors` without stopping the others.
pub async fn record(arguments: Arguments) -> Result<(), RecorderError> {
    let archive = RecordingArchive::create(&arguments.directory)?;
    let interruption_marker = arguments.directory.join(CONTROLLER_INTERRUPTION_MARKER);
    archive.metadata(&arguments.room)?;
    let stopped = CancellationToken::new();
    let mut interruption =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
    let mut termination =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let signal_stopped = stopped.clone();
    let signal_task = tokio::spawn(async move {
        tokio::select! { _ = interruption.recv() => {}, _ = termination.recv() => {} }
        signal_stopped.cancel();
    });
    let supervisor_lost = Arc::new(AtomicBool::new(false));
    let supervisor_task = arguments.supervisor_pid.map(|supervisor_pid| {
        tokio::spawn(watch_supervisor(
            supervisor_pid,
            supervisor_lost.clone(),
            stopped.clone(),
        ))
    });
    let hardware = tokio::task::spawn_blocking(hardware_encoder_available).await?;
    eprintln!(
        "Recording encoder: {}",
        if hardware {
            "Intel VAAPI"
        } else {
            "software H.264"
        }
    );
    let state_archive = archive.clone();
    let state_stopped = stopped.clone();
    let state_task = tokio::spawn(async move {
        let result = record_station_state(
            &arguments.station_url,
            &state_archive,
            state_stopped.clone(),
        )
        .await;
        state_stopped.cancel();
        result
    });
    let result = record_cameras(&arguments.room, &archive, hardware, stopped.clone()).await;
    stopped.cancel();
    let state_result = state_task.await;
    signal_task.abort();
    if let Some(supervisor_task) = supervisor_task {
        supervisor_task.abort();
    }
    let lost_supervisor = supervisor_lost.load(Ordering::SeqCst);
    let state_result = state_result
        .map_err(RecorderError::from)
        .and_then(|state| state);
    let interruption_marker_exists = interruption_marker.try_exists()?;
    let controller_interrupted = archive.controller_interrupted()? || interruption_marker_exists;
    if interruption_marker_exists {
        archive.json("/station/recording_errors", &json!({"source": "controller", "detail": "Motor controller unavailable during recording"}), unix_nanoseconds())?;
    }
    let status = if lost_supervisor {
        json!({"status": "incomplete", "reason": "The supervising station exited; the recording ended before it was asked to stop"})
    } else if let Some(error) = result.as_ref().err().or(state_result.as_ref().err()) {
        json!({"status": "incomplete", "reason": error.to_string()})
    } else if controller_interrupted {
        json!({"status": "incomplete", "reason": "The motor controller was unavailable during the recording; arm telemetry has a gap"})
    } else {
        json!({"status": "complete"})
    };
    // INVARIANT: Written last, so a reader sees how the recording ended; a file without it was killed.
    let status_result = archive.json("/recording/status", &status, unix_nanoseconds());
    let finish_result = archive.finish();
    if lost_supervisor {
        // WHY: Distinct from a requested stop, so the exit status says it ended early.
        finish_result?;
        status_result?;
        return Err(RecorderError::SupervisorLost);
    }
    if controller_interrupted {
        finish_result?;
        status_result?;
        return Err(RecorderError::ControllerInterrupted);
    }
    result?;
    state_result?;
    status_result?;
    finish_result
}

async fn record_station_state(
    station_url: &str,
    archive: &RecordingArchive,
    stopped: CancellationToken,
) -> Result<(), RecorderError> {
    crate::station::subscribe(station_url, Some(archive), stopped).await
}

async fn record_cameras(
    room: &str,
    archive: &RecordingArchive,
    hardware: bool,
    stopped: CancellationToken,
) -> Result<(), RecorderError> {
    let mut backoff = hebbian_contracts::Backoff::new(Duration::from_secs(3));
    loop {
        let attempt_started = std::time::Instant::now();
        let result = async {
            let credentials = station_access::Credentials::from_environment()?;
            let access = credentials.watcher_access(room, station_access::WatcherRole::Recorder)?;
            Ok::<(), RecorderError>(
                record_room(
                    &access.server_url,
                    &access.token,
                    &(std::sync::Arc::new(archive.clone()) as std::sync::Arc<dyn VideoOutput>),
                    hardware,
                    stopped.child_token(),
                )
                .await?,
            )
        }
        .await;
        if stopped.is_cancelled() {
            return Ok(());
        }
        if let Err(error) = result {
            eprintln!("cameras unavailable: {error}");
            archive.json(
                "/station/recording_errors",
                &json!({"detail": format!("cameras unavailable: {error}")}),
                unix_nanoseconds(),
            )?;
        }
        tokio::select! {
            () = stopped.cancelled() => return Ok(()),
            () = tokio::time::sleep(backoff.next_delay(attempt_started.elapsed())) => {},
        }
    }
}

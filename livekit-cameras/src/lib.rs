pub mod error;
#[doc(hidden)]
pub mod parent_process;
pub mod recorder;

pub(crate) mod station;

/// Follow station telemetry independently of `LiveKit` publishing.
/// # Errors
/// Returns local telemetry errors; connection loss is retried with resume.
pub async fn follow_station(
    url: &str,
    stopped: tokio_util::sync::CancellationToken,
) -> Result<(), recorder::RecorderError> {
    station::subscribe(url, None, stopped).await
}

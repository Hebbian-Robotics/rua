pub mod error;
pub mod recorder;
#[doc(hidden)]
pub mod supervisor;

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

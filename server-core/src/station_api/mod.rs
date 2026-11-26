mod admission;
mod artifact_workspace;
mod commands;
mod events;
mod executions;
mod http;
mod http_state;
mod interlocks;
mod model_assets;
mod program_http;
mod programs;
mod runtime;
mod sequence_images;
mod state;
mod submissions;
mod telemetry;

pub use http::{HttpServer, trace_directories};
pub use sequence_images::SequenceCameraSource;
pub use state::station_state;
pub use telemetry::{DiscardTelemetry, RunReplayWindow, RunTelemetry, RunUpdates};

fn parse_operation_id(
    value: &str,
) -> Result<submissions::OperationId, (axum::http::StatusCode, String)> {
    submissions::OperationId::parse(value).ok_or((
        axum::http::StatusCode::BAD_REQUEST,
        "Expected a UUID v4 operation ID".to_owned(),
    ))
}

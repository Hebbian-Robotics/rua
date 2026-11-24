#[derive(Debug, thiserror::Error)]
pub enum RecorderError {
    #[error("video recorder: {0}")]
    Video(#[from] livekit_camera_recorder::RecorderError),
    #[error("station HTTP: {0}")]
    Http(#[from] reqwest::Error),
    #[error(transparent)]
    Access(#[from] station_access::AccessError),
    #[error("LiveKit room: {0}")]
    Room(#[from] livekit::RoomError),
    #[error("recording I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("MCAP: {0}")]
    Mcap(#[from] mcap::McapError),
    #[error("JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("recording task: {0}")]
    Task(#[from] tokio::task::JoinError),
    #[error("camera: {0}")]
    Camera(#[from] cameras::CameraError),
    #[error("invalid recording data: {0}")]
    InvalidData(String),
    #[error("{operation} exited with {status}")]
    Process {
        operation: &'static str,
        status: std::process::ExitStatus,
    },
    #[error("{0} timed out")]
    Timeout(&'static str),
    #[error("the supervising station exited; the recording was finalized as incomplete")]
    StationExited,
    #[error("MCAP writer lock was poisoned")]
    WriterPoisoned,
}

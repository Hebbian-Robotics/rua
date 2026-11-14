#[derive(Debug, thiserror::Error)]
pub enum PublisherError {
    #[error(transparent)]
    NativePublisher(#[from] livekit_camera_publisher::error::PublisherError),
    #[error(transparent)]
    Recorder(#[from] crate::recorder::RecorderError),
    #[error(transparent)]
    Access(#[from] station_access::AccessError),
    #[error(transparent)]
    Camera(#[from] cameras::CameraError),
    #[error("LiveKit room: {0}")]
    Room(#[from] livekit::RoomError),
    #[error("invalid configuration: {0}")]
    Configuration(String),
    #[error("system operation: {0}")]
    Io(#[from] std::io::Error),
    #[error("publisher task: {0}")]
    Task(#[from] tokio::task::JoinError),
    #[error("all camera publishers stopped")]
    NoActiveCameras,
}

#[derive(Debug, thiserror::Error)]
pub enum PublisherError {
    #[error(transparent)]
    InvalidCameraName(#[from] hebbian_contracts::InvalidCameraName),
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
    #[error("system operation: {0}")]
    Io(#[from] std::io::Error),
}

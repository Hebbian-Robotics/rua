mod library;
mod routes;
pub use hebbian_contracts::{Recording, Recordings};
pub use library::RecordingLibrary;
pub use routes::router;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("No such recording")]
    NotFound,
    #[error("Recording is still in progress")]
    RecordingInProgress,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Task(#[from] tokio::task::JoinError),
}

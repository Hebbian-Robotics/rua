use crate::{Error, RecordingLibrary};
use axum::{
    Json, Router,
    body::Body,
    extract::{Path, State},
    http::{Request, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
};
use tower::ServiceExt;
use tower_http::services::ServeFile;
// CONSTRAINT: Routes are relative; the station nests this under `/viewer/api`.
pub fn router(library: RecordingLibrary) -> Router {
    Router::new()
        .route("/recordings", get(list))
        .route("/recordings/{*path}", get(file).delete(delete))
        .with_state(library)
}
impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let status = match self {
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::RecordingInProgress => StatusCode::CONFLICT,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (
            status,
            Json(serde_json::json!({ "detail": self.to_string() })),
        )
            .into_response()
    }
}
async fn list(State(library): State<RecordingLibrary>) -> Result<impl IntoResponse, Error> {
    Ok(Json(
        tokio::task::spawn_blocking(move || library.recordings()).await??,
    ))
}
async fn delete(
    State(library): State<RecordingLibrary>,
    Path(name): Path<String>,
) -> Result<impl IntoResponse, Error> {
    tokio::task::spawn_blocking(move || library.delete_recording(&name)).await??;
    Ok(StatusCode::NO_CONTENT)
}
async fn file(
    State(library): State<RecordingLibrary>,
    Path(path): Path<String>,
    request: Request<Body>,
) -> Result<Response, Error> {
    let path = tokio::task::spawn_blocking(move || library.recording_file(&path)).await??;
    serve(path, request).await
}
async fn serve(path: std::path::PathBuf, request: Request<Body>) -> Result<Response, Error> {
    match ServeFile::new(path).oneshot(request).await {
        Ok(response) => Ok(response.map(Body::new)),
        Err(error) => match error {},
    }
}

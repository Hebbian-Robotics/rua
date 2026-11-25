//! The station's HTTP routes: the generated per-route methods (`generated/http_routes.rs`)
//! and the few that read more than a body.

#[path = "generated/http_routes.rs"]
mod routes;

use hebbian_contracts::MediaStatusReport;
use reqwest::{Method, Response, Url};
use serde::Serialize;

use super::Station;
use crate::error::{Error, Result};

/// One JPEG frame and when it was captured.
#[derive(Debug, Clone)]
pub struct CameraJpeg {
    pub jpeg: Vec<u8>,
    /// Receipt time for simulator cameras; none when the service sent no timestamp.
    pub captured_at_unix_ns: Option<u64>,
}

const CAPTURED_AT_HEADER: &str = "X-Captured-At-Unix-Ns";

fn endpoint(base: &str, segments: &[&str]) -> Result<Url> {
    let mut url =
        Url::parse(base).map_err(|error| Error::Connection(format!("{base}: {error}")))?;
    url.path_segments_mut()
        .map_err(|()| Error::Connection(format!("{base} cannot be a base URL")))?
        .pop_if_empty()
        .extend(segments);
    Ok(url)
}

async fn checked(response: Response) -> Result<Response> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    Err(Error::Http {
        status: status.as_u16(),
        body: response.text().await.unwrap_or_default(),
    })
}

impl Station {
    /// One request to `base`, with a JSON body when there is one.
    async fn request(
        &self,
        method: Method,
        base: &str,
        segments: &[&str],
        body: Option<&(impl Serialize + ?Sized)>,
    ) -> Result<Response> {
        let request = self.http.request(method, endpoint(base, segments)?);
        let request = match body {
            Some(body) => request.json(body),
            None => request,
        };
        checked(request.send().await?).await
    }

    /// Names of the cameras streaming now.
    ///
    /// # Errors
    /// [`Error::Connection`] when the media service is unreachable.
    pub async fn camera_names(&self) -> Result<Vec<String>> {
        Ok(self
            .list_cameras()
            .await?
            .into_iter()
            .filter(|camera| camera.available)
            .map(|camera| camera.name)
            .collect())
    }

    /// The newest JPEG frame from a camera.
    ///
    /// # Errors
    /// [`Error::CameraUnavailable`] when the media service has no fresh frame for the camera.
    pub async fn camera_jpeg(&self, name: &str) -> Result<CameraJpeg> {
        self.camera_jpeg_bounded(name, None).await
    }

    /// Fetch a frame, enforcing the caller's byte limit as each chunk arrives.
    ///
    /// # Errors
    /// Returns [`Error::FrameTooLarge`] when the response exceeds the byte limit, or the errors from [`Self::camera_jpeg`].
    pub async fn camera_jpeg_bounded(
        &self,
        name: &str,
        maximum_bytes: Option<usize>,
    ) -> Result<CameraJpeg> {
        let mut response = match self.get_camera(name).await {
            Err(Error::Http { status: 404, .. }) => {
                return Err(Error::CameraUnavailable {
                    name: name.to_owned(),
                    listed: self.list_cameras().await.unwrap_or_default(),
                    last_error: self.camera_problem(name).await,
                });
            }
            other => other?,
        };
        let captured_at_unix_ns = response
            .headers()
            .get(CAPTURED_AT_HEADER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse().ok());
        let mut jpeg = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            if let Some(limit) = maximum_bytes
                && chunk.len() > limit.saturating_sub(jpeg.len())
            {
                return Err(Error::FrameTooLarge(limit));
            }
            jpeg.extend_from_slice(&chunk);
        }
        Ok(CameraJpeg {
            jpeg,
            captured_at_unix_ns,
        })
    }

    async fn camera_problem(&self, name: &str) -> Option<String> {
        let status: MediaStatusReport = self
            .request(Method::GET, &self.server, &["media", "status"], None::<&()>)
            .await
            .ok()?
            .json()
            .await
            .ok()?;
        status.cameras.get(name)?.snapshot_problem()
    }

    /// A trace file's bytes exactly as served: gzip-compressed.
    ///
    /// # Errors
    /// [`Error::Http`] with 404 for an unknown name.
    pub async fn download_trace(&self, name: &str) -> Result<Vec<u8>> {
        Ok(self.get_trace_file(name).await?.bytes().await?.to_vec())
    }
}

use std::path::Path;
use std::sync::Arc;

use axum::extract::{Path as UrlPath, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};

use super::http_state::StationHttpState;

pub(super) async fn read_model_asset(
    State(station_state): State<Arc<StationHttpState>>,
    UrlPath((arm, asset)): UrlPath<(String, String)>,
) -> Response {
    let Some(link) = station_state.arms().and_then(|arms| arms.get(&arm)) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let urdf = link.config().urdf.clone();
    match tokio::task::spawn_blocking(move || model_asset(Path::new(&urdf), &asset)).await {
        Ok(Ok((content, content_type))) => (
            [
                (header::CONTENT_TYPE, content_type),
                (header::CACHE_CONTROL, "no-cache"),
            ],
            content,
        )
            .into_response(),
        Ok(Err(status)) => status.into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

fn model_asset(urdf: &Path, asset: &str) -> Result<(Vec<u8>, &'static str), StatusCode> {
    if asset == "robot.urdf" {
        return std::fs::read(urdf)
            .map(|content| (content, "application/xml"))
            .map_err(|_| StatusCode::NOT_FOUND);
    }
    let description = urdf_rs::read_file(urdf).map_err(|_| StatusCode::NOT_FOUND)?;
    let referenced = description.links.iter().any(|link| {
        link.visual.iter().map(|visual| &visual.geometry)
            .chain(link.collision.iter().map(|collision| &collision.geometry))
            .any(|geometry| matches!(geometry, urdf_rs::Geometry::Mesh { filename, .. } if filename == asset))
    });
    if !referenced {
        return Err(StatusCode::NOT_FOUND);
    }
    let directory = urdf
        .parent()
        .ok_or(StatusCode::NOT_FOUND)?
        .canonicalize()
        .map_err(|_| StatusCode::NOT_FOUND)?;
    let path = directory
        .join(asset)
        .canonicalize()
        .map_err(|_| StatusCode::NOT_FOUND)?;
    if !path.starts_with(&directory) {
        return Err(StatusCode::NOT_FOUND);
    }
    std::fs::read(path)
        .map(|content| (content, "application/octet-stream"))
        .map_err(|_| StatusCode::NOT_FOUND)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serves_the_control_model_and_its_referenced_meshes_without_exposing_other_files() {
        let directory =
            std::env::temp_dir().join(format!("model-assets-{}", crate::arm::new_identifier()));
        std::fs::create_dir_all(directory.join("meshes")).unwrap();
        let urdf = directory.join("selected.urdf");
        let description = br#"<robot name="selected"><link name="gripper">
          <visual><geometry><mesh filename="meshes/finger.stl"/></geometry></visual>
          </link></robot>"#;
        std::fs::write(&urdf, description).unwrap();
        std::fs::write(directory.join("meshes/finger.stl"), b"finger geometry").unwrap();
        std::fs::write(directory.join("private.txt"), b"not a model asset").unwrap();
        assert_eq!(model_asset(&urdf, "robot.urdf").unwrap().0, description);
        assert_eq!(
            model_asset(&urdf, "meshes/finger.stl").unwrap().0,
            b"finger geometry"
        );
        assert_eq!(
            model_asset(&urdf, "private.txt"),
            Err(StatusCode::NOT_FOUND)
        );
        assert_eq!(
            model_asset(&urdf, "../private.txt"),
            Err(StatusCode::NOT_FOUND)
        );
        std::fs::remove_dir_all(directory).unwrap();
    }
}

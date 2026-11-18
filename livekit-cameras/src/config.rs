use crate::error::PublisherError;
use cameras::config::configure_camera;
use clap::Args;
use std::collections::BTreeMap;

#[derive(Args)]
#[command(about = "Publish native cameras into a LiveKit room, one participant per camera")]
pub struct Arguments {
    #[arg(long)]
    pub room: String,
    #[command(flatten)]
    pub publishing: livekit_camera_publisher::config::PublishingOptions,
    #[arg(long, default_value = "http://127.0.0.1:8792")]
    pub executor_url: String,
    /// Camera mapping: name=device, repeatable.
    #[arg(long = "camera")]
    camera_mappings: Vec<String>,
}

pub use livekit_camera_publisher::config::CameraConfiguration;

impl Arguments {
    pub fn cameras(&self) -> Result<Vec<CameraConfiguration>, PublisherError> {
        if self.room.is_empty() {
            return Err(PublisherError::Configuration(
                "room cannot be empty".to_owned(),
            ));
        }
        let mut cameras = BTreeMap::new();
        for specification in &self.camera_mappings {
            configure_camera(&mut cameras, specification)?;
        }
        if cameras.is_empty() {
            return Err(PublisherError::Configuration(
                "provide at least one --camera name=device".to_owned(),
            ));
        }
        cameras
            .into_iter()
            .map(|(name, device)| {
                let mut camera = CameraConfiguration::new(name, device)?;
                camera.publishing = self.publishing;
                Ok(camera)
            })
            .collect()
    }
}

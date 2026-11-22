use crate::error::PublisherError;
use cameras::config::configure_camera;
use clap::Args;
use std::{collections::BTreeMap, net::SocketAddr, path::PathBuf};

#[derive(Args)]
#[command(about = "Serve camera snapshots and publish to LiveKit when a room is configured")]
pub struct ServeArguments {
    #[arg(long, default_value = "127.0.0.1:8781")]
    pub address: SocketAddr,
    /// Publish native cameras to this room and subscribe for snapshots.
    #[arg(long, value_parser = clap::builder::NonEmptyStringValueParser::new())]
    pub room: Option<String>,
    #[command(flatten)]
    pub publishing: livekit_camera_publisher::config::PublishingOptions,
    #[arg(long, default_value = "http://127.0.0.1:8792")]
    pub station_url: String,
    /// Exit if the station that launched this service exits.
    #[arg(long)]
    pub station_pid: Option<u32>,
    /// Unix socket exposed by the simulator.
    #[arg(long)]
    pub camera_socket: Option<PathBuf>,
    /// Native camera mapping (name=device), repeatable.
    #[arg(long = "camera")]
    camera_mappings: Vec<String>,
}

pub use livekit_camera_publisher::config::CameraConfiguration;

impl ServeArguments {
    pub fn cameras(&self) -> Result<Vec<CameraConfiguration>, PublisherError> {
        let mut cameras = BTreeMap::new();
        for specification in &self.camera_mappings {
            configure_camera(&mut cameras, specification)?;
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

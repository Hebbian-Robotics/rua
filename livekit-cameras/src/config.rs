use crate::{
    error::PublisherError,
    usb_camera::{self, UsbCameraIdentity},
};
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
    /// Automatic native camera mapping (`name=VID:PID:USB_SERIAL`), repeatable.
    #[arg(long = "usb-camera")]
    usb_cameras: Vec<String>,
}

pub use livekit_camera_publisher::config::CameraConfiguration;

enum CameraDevice {
    Explicit(String),
    Usb(UsbCameraIdentity),
}

pub(crate) struct CameraInput {
    name: String,
    device: CameraDevice,
    publishing: livekit_camera_publisher::config::PublishingOptions,
}

impl CameraInput {
    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    pub(crate) async fn resolve(&self) -> Result<CameraConfiguration, PublisherError> {
        let device = match &self.device {
            CameraDevice::Explicit(device) => device.clone(),
            CameraDevice::Usb(identity) => {
                let identity = identity.clone();
                tokio::task::spawn_blocking(move || usb_camera::resolve(&identity))
                    .await
                    .map_err(std::io::Error::other)??
            }
        };
        let mut camera = CameraConfiguration::new(self.name.clone(), device)?;
        camera.publishing = self.publishing;
        Ok(camera)
    }
}

impl ServeArguments {
    pub(crate) fn cameras(&self) -> Result<Vec<CameraInput>, PublisherError> {
        let mut automatic = BTreeMap::new();
        let mut explicit = BTreeMap::new();
        for specification in &self.usb_cameras {
            configure_camera(&mut automatic, specification)?;
        }
        for specification in &self.camera_mappings {
            configure_camera(&mut explicit, specification)?;
        }
        automatic.retain(|name, _| !explicit.contains_key(name));
        explicit
            .into_iter()
            .map(|(name, device)| Ok((name, CameraDevice::Explicit(device))))
            .chain(
                automatic
                    .into_iter()
                    .map(|(name, identity)| Ok((name, CameraDevice::Usb(identity.parse()?)))),
            )
            .map(|result: Result<_, std::io::Error>| {
                let (name, device) = result?;
                Ok(CameraInput {
                    name,
                    device,
                    publishing: self.publishing,
                })
            })
            .collect()
    }
}

use std::net::SocketAddr;
use std::path::PathBuf;

#[derive(clap::Args)]
pub(crate) struct ServeArguments {
    /// Bundled profile name, absolute local profile path, or auto for a registered SO-101.
    #[arg(long, default_value = "auto")]
    pub(crate) profile: String,
    #[command(flatten)]
    pub(crate) selection: super::SelectionArguments,
    /// An arm to serve; repeat for several. Default: all of the profile's arms.
    #[arg(long = "arm")]
    pub(crate) arms: Vec<String>,
    /// Defaults to `$HEBBIAN_PROFILE_DIRECTORY`, else installed or checkout
    /// profiles above the server binary or the working directory.
    #[arg(long)]
    pub(crate) profile_directory: Option<PathBuf>,
    /// Drive a stand-in with no physics instead of the profile's hardware or simulator: joints ease
    /// toward their commands with no contacts, table, gravity or loads. For exercising the station,
    /// not for judging motion.
    #[arg(long)]
    pub(crate) no_physics_stand_in: bool,
    /// Serve the station HTTP API and event stream on this address.
    #[arg(long, default_value = "127.0.0.1:8792")]
    pub(crate) http_address: String,
    /// Serve camera snapshots and recordings (`hebbian-rua-media serve`) on this address.
    #[arg(long, default_value = "127.0.0.1:8781")]
    pub(crate) media_address: SocketAddr,
    /// `name=device`, repeated: a USB camera besides the profile's, a video
    /// device path or an index; `name=none` drops a profile camera.
    #[arg(long = "camera", value_parser = parse_camera)]
    pub(crate) cameras: Vec<(String, Option<String>)>,
    /// Where the arms' configs and a simulator's plant sockets go, as
    /// `NAME.plant.sock`. Default: a new short directory in `/tmp`, so the socket paths fit.
    #[arg(long)]
    pub(crate) work_directory: Option<PathBuf>,
    /// Where the cycle traces go. Default: the config's own.
    #[arg(long)]
    pub(crate) trace_directory: Option<PathBuf>,
    /// How long to wait for your simulator to connect to each plant socket.
    #[arg(long, default_value_t = 30.0)]
    pub(crate) simulator_connect_seconds: f64,
    /// How long your simulator may take to answer one step before the arm trips.
    #[arg(long, default_value_t = 5.0)]
    pub(crate) simulator_reply_seconds: f64,
}

fn parse_camera(text: &str) -> Result<(String, Option<String>), String> {
    match text.split_once('=') {
        Some((name, "none")) if !name.is_empty() => Ok((name.to_owned(), None)),
        Some((name, device)) if !name.is_empty() && !device.is_empty() => {
            Ok((name.to_owned(), Some(device.to_owned())))
        }
        _ => Err("expected NAME=DEVICE or NAME=none".to_owned()),
    }
}

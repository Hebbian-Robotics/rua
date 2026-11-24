use std::net::SocketAddr;
use std::path::PathBuf;

#[derive(clap::Args)]
pub(crate) struct ServeArguments {
    /// Bundled profile name, absolute local profile path, or auto for a registered SO-101.
    ///
    /// Use a real arm whenever available. Use `--profile so101-sim-only-no-real-arm`
    /// for testing with a simulator when hardware is unavailable.
    #[arg(long, default_value = "auto")]
    pub(crate) profile: String,
    #[command(flatten)]
    pub(crate) selection: crate::so101::SelectionArguments,
    /// An arm to serve; repeat for several. Default: all of the profile's arms.
    #[arg(long = "arm")]
    pub(crate) arms: Vec<String>,
    /// Defaults to `$HEBBIAN_PROFILE_DIRECTORY`, else installed or checkout
    /// profiles above the server binary or the working directory.
    #[arg(long)]
    pub(crate) profile_directory: Option<PathBuf>,
    /// Use a stand-in with no physics for station and API debugging only.
    ///
    /// Use a real arm whenever available. The stand-in eases joints toward their commands and
    /// ignores contacts, the table, gravity and loads.
    #[arg(long)]
    pub(crate) no_physics_stand_in: bool,
    /// Serve the station HTTP API and event stream on this address.
    #[arg(long, default_value = "127.0.0.1:8792")]
    pub(crate) http_address: String,
    /// Serve camera snapshots and recordings (`hebbian-rua-media serve`) on this address.
    #[arg(long, default_value = "127.0.0.1:8781")]
    pub(crate) media_address: SocketAddr,
    /// Add or override a USB camera as `name=device`. Repeat for multiple cameras.
    ///
    /// Run `rua camera-devices` for device paths or indices. For example, pass
    /// `--camera wrist=2 --camera overhead=4`. Use `name=none` to drop a profile camera.
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

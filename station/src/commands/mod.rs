//! The command line: its arguments and commands.
mod serve;
pub(crate) mod so101;
pub(crate) use serve::ServeArguments;
pub(crate) use so101::SelectionArguments;

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(about = "Run an SO-101 station for robot-use clients")]
pub(crate) struct Arguments {
    #[command(subcommand)]
    pub(crate) command: Command,
}

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Serve the station for a robot profile: build each arm's config, drive its
    /// servos, an external simulator or the no-physics stand-in, and serve the HTTP API,
    /// recordings and lease routes.
    Serve {
        #[command(flatten)]
        arguments: serve::ServeArguments,
        #[command(flatten)]
        web: WebArguments,
    },
    /// Internal motor-controller child, started by the station supervisor.
    #[command(hide = true)]
    Controller {
        #[arg(long = "arm", required = true, value_parser = parse_arm)]
        arms: Vec<(String, PathBuf)>,
        #[arg(long)]
        station_socket: PathBuf,
        #[arg(long)]
        station_pid: u32,
        #[arg(long)]
        instance: uuid::Uuid,
        #[arg(long)]
        socket_directory: Option<PathBuf>,
        #[arg(long, default_value_t = crate::run::DEFAULT_STAND_IN_LAG_SECONDS)]
        plant_lag_seconds: f64,
        #[arg(long)]
        hardware: bool,
    },
    /// List USB adapter serials and their SO-101 config registry matches.
    /// Reads USB metadata only; never opens serial devices or changes torque.
    So101Discover {
        /// Local SO-101 config checkout; defaults to the configured registry.
        #[arg(long)]
        so101_config_directory: Option<PathBuf>,
    },
    /// Check station connectivity, arm feedback, `LiveKit` and an optional camera snapshot.
    Status {
        /// HTTP address of the running station.
        #[arg(
            long,
            env = "HEBBIAN_STATION_URL",
            default_value = "http://127.0.0.1:8792"
        )]
        server: reqwest::Url,
        /// Direct camera service address. Defaults to the station URL.
        #[arg(long, env = "HEBBIAN_MEDIA_SERVER")]
        media_server: Option<reqwest::Url>,
        /// Camera whose latest frame must be available.
        #[arg(long)]
        camera: Option<String>,
    },
    /// List local video capture devices and their selectors for serve --camera.
    CameraDevices {
        /// Media companion to run. Defaults to the companion selected by serve.
        #[arg(long, env = "HEBBIAN_MEDIA")]
        media_executable: Option<PathBuf>,
    },
    /// Check the servos against the profile and print one reading, with torque
    /// untouched.
    CheckArm {
        #[arg(long)]
        config: PathBuf,
    },
    /// Turn torque off, which drops the arm.
    Disable {
        #[arg(long)]
        config: PathBuf,
    },
    /// Print the lowest point of the moving geometry for sampled joints, for
    /// comparing the table guard's model of the arm with another one.
    ProbeRobot {
        #[arg(long)]
        trace: PathBuf,
    },
    /// Run recorded observations through the ONNX policy, for comparing against
    /// a reference implementation.
    ReplayPolicy {
        #[arg(long)]
        trace: PathBuf,
    },
    /// Internal watchdog for an enabled arm. The controller sends heartbeats on stdin.
    Guard {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        controller_pid: u32,
    },
    /// Write the station's `OpenAPI` document to stdout, or the simulator socket schema.
    Schema {
        #[arg(long, conflicts_with = "openapi")]
        sim_plant: bool,
        /// The `OpenAPI` 3.1 document: the one description of the station's API.
        #[arg(long)]
        openapi: bool,
    },
    /// Build one arm's config from a robot profile and print it as JSON. Joint
    /// limits come from the arm's `LeRobot` calibration on a Feetech profile,
    /// unless `--urdf-limits`.
    Config {
        /// Bundled profile name, absolute local profile path, or auto for a registered SO-101.
        #[arg(long)]
        profile: String,
        #[command(flatten)]
        selection: so101::SelectionArguments,
        #[arg(long)]
        arm: String,
        /// Defaults to `$HEBBIAN_PROFILE_DIRECTORY`, else installed or checkout
        /// profiles above the server binary or the working directory.
        #[arg(long)]
        profile_directory: Option<PathBuf>,
        /// Use the URDF's joint limits, as in simulation.
        #[arg(long)]
        urdf_limits: bool,
    },
    /// Parse and validate a config file, then exit.
    CheckConfig {
        #[arg(long)]
        config: PathBuf,
    },
}

/// What the HTTP server serves beside control: recordings and `LiveKit` access. The
/// `LiveKit` credentials come from `LIVEKIT_URL`, `LIVEKIT_API_KEY` and
/// `LIVEKIT_API_SECRET`; without them `LiveKit` cameras are unavailable.
#[derive(clap::Args)]
#[expect(clippy::struct_excessive_bools, reason = "Independent CLI switches")]
pub(crate) struct WebArguments {
    /// Stream and capture snapshots from local cameras only. Disables remote cameras.
    ///
    /// Use this fallback when `LiveKit` is unavailable and you cannot run a local
    /// `livekit-server`. Open the RUA example viewer to watch local USB cameras. For
    /// remote cameras, configure `LIVEKIT_URL`, `LIVEKIT_API_KEY` and `LIVEKIT_API_SECRET`
    /// in the environment or `.env`.
    #[arg(
        long = "local-only-no-remote-cameras",
        aliases = ["local-cameras-only", "local-cameras-only-no-stream-preview"],
        conflicts_with = "enable_recording"
    )]
    local_cameras_only: bool,
    /// Where `hebbian-rua-media` writes recordings, one directory per recording.
    #[arg(
        long,
        env = "HEBBIAN_RECORDINGS_DIRECTORY",
        default_value = "recordings"
    )]
    recordings_directory: PathBuf,
    /// Enable recording; requires `LiveKit` and `FFmpeg` (see docs/recording.md).
    #[arg(long, env = "HEBBIAN_ENABLE_RECORDING", default_value_t = false)]
    enable_recording: bool,
    /// The installed `hebbian-rua-media`, which records the cameras.
    /// Default: beside this executable, else on `PATH`, else `target/debug`.
    #[arg(long, env = "HEBBIAN_MEDIA")]
    media_executable: Option<PathBuf>,
    /// Serve camera snapshots only on the media companion's address.
    #[arg(long)]
    disable_camera_forwarding: bool,
    /// The `LiveKit` room every camera publishes to; a new one each start by default.
    #[arg(long, env = "HEBBIAN_LIVEKIT_ROOM")]
    room: Option<String>,
    /// The page a phone invite opens; the room's access goes in its fragment.
    #[arg(
        long,
        env = "HEBBIAN_PHONE_PAGE_URL",
        default_value = hebbian_rua_server::web::DEFAULT_PHONE_PAGE_URL
    )]
    phone_page_url: String,
    /// Show the viewer's manual pose controls (a single SO-101 arm).
    #[arg(long)]
    manual_control: bool,
}

impl WebArguments {
    pub(crate) fn settings(self) -> hebbian_rua_server::web::WebSettings {
        use hebbian_rua_server::web::{CameraForwarding, WebSettings};

        WebSettings {
            local_cameras_only: self.local_cameras_only,
            recordings_directory: self.recordings_directory,
            enable_recording: self.enable_recording,
            media_address: None,
            camera_forwarding: if self.disable_camera_forwarding {
                CameraForwarding::DirectOnly
            } else {
                CameraForwarding::Enabled
            },
            media_executable: crate::media::find_media(self.media_executable),
            room: self.room,
            phone_page_url: self.phone_page_url,
            manual_control: self.manual_control,
            media_status: hebbian_rua_server::media_status::MediaStatus::default(),
        }
    }
}

fn parse_arm(text: &str) -> Result<(String, PathBuf), String> {
    match text.split_once('=') {
        Some((name, path)) if !name.is_empty() && !path.is_empty() => {
            Ok((name.to_owned(), PathBuf::from(path)))
        }
        _ => Err("expected NAME=CONFIG.json".to_owned()),
    }
}

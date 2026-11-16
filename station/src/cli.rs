//! The command line: its arguments and commands.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(about = "The real-time control loop for the SO-101")]
pub(crate) struct Arguments {
    #[command(subcommand)]
    pub(crate) command: Command,
}

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Serve the station for a robot profile: build each arm's config, drive its
    /// servos, a connected simulator or the lag simulator, and serve the HTTP API,
    /// the viewer, recording and lease routes over HTTP.
    Serve {
        #[command(flatten)]
        arguments: crate::serve::ServeArguments,
        #[command(flatten)]
        web: WebArguments,
    },
    /// Run the control loop for one or more arms in lockstep and print each
    /// arm's cycle timing. With `--http-address` it serves the HTTP API and follows the
    /// station's goals; otherwise it sweeps a sine.
    Run {
        /// An arm and its config, as `NAME=CONFIG.json`; repeat for each arm.
        /// A grouped move names these arms.
        #[arg(long = "arm", required = true, value_parser = parse_arm)]
        arms: Vec<(String, PathBuf)>,
        /// Stop after this many seconds; without it, run until SIGINT or SIGTERM.
        #[arg(long)]
        seconds: Option<f64>,
        /// Where a simulator's plant sockets go, as `NAME.plant.sock`.
        #[arg(long)]
        socket_directory: Option<PathBuf>,
        /// How slowly the simulated joints follow commands.
        #[arg(long, default_value_t = 0.05)]
        plant_lag_seconds: f64,
        /// Drive the Feetech servos in the config's `feetech` section instead of
        /// the simulator: check them, turn torque on, and leave it on when the
        /// loop ends, so the arm holds.
        #[arg(long)]
        hardware: bool,
        /// Serve HTTP (health and the cycle traces) on this address, such as
        /// `127.0.0.1:8792`, on threads of its own beside the loop.
        #[arg(long)]
        http_address: Option<String>,
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
        instance: uuid::Uuid,
        #[arg(long)]
        socket_directory: Option<PathBuf>,
        #[arg(long, default_value_t = 0.05)]
        plant_lag_seconds: f64,
        #[arg(long)]
        hardware: bool,
    },
    /// List USB controller serials and their SO-101 config registry matches.
    /// Reads USB metadata only; never opens serial devices or changes torque.
    So101Discover {
        /// Local SO-101 config checkout; defaults to the configured registry.
        #[arg(long)]
        so101_config_directory: Option<PathBuf>,
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
    /// Watch an enabled arm for an executor: started by the executor, not for direct
    /// use. Reads heartbeats on stdin.
    Guard {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        executor_pid: u32,
    },
    /// Write the contract's JSON Schema (requests, frames, config) to stdout, or with
    /// `--sim-plant` the schema of the simulator socket's messages, or with `--openapi` the
    /// `OpenAPI` document of the station HTTP API.
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
        #[arg(
            long,
            required_unless_present = "profile_json",
            conflicts_with = "profile_json"
        )]
        profile: Option<String>,
        #[command(flatten)]
        selection: crate::so101::SelectionArguments,
        /// Read the profile from stdin as JSON with the TOML keys and no
        /// `extends`; relative paths resolve against the working directory.
        #[arg(long)]
        profile_json: bool,
        #[arg(long)]
        arm: String,
        /// Defaults to `$HEBBIAN_PROFILE_DIRECTORY`, else installed or checkout
        /// profiles above the executor or the working directory.
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
pub(crate) struct WebArguments {
    /// Where `hebbian-robot-use-media` writes recordings, one directory per recording.
    #[arg(
        long,
        env = "HEBBIAN_RECORDINGS_DIRECTORY",
        default_value = "recordings"
    )]
    recordings_directory: PathBuf,
    /// Enable recording; requires `LiveKit` and `FFmpeg` (see docs/recording.md).
    #[arg(long, env = "HEBBIAN_ENABLE_RECORDING", default_value_t = false)]
    enable_recording: bool,
    /// The installed `hebbian-robot-use-media`, which records the cameras.
    /// Default: beside this executable, else on `PATH`, else `target/debug`.
    #[arg(long, env = "HEBBIAN_MEDIA")]
    media_executable: Option<PathBuf>,
    /// The `LiveKit` room every camera publishes to; a new one each start by default.
    #[arg(long, env = "HEBBIAN_LIVEKIT_ROOM")]
    room: Option<String>,
    /// The page a phone invite opens; the room's access goes in its fragment.
    #[arg(
        long,
        env = "HEBBIAN_PHONE_PAGE_URL",
        default_value = hebbian_station::web::DEFAULT_PHONE_PAGE_URL
    )]
    phone_page_url: String,
    /// Show the viewer's manual pose controls (a single SO-101 arm).
    #[arg(long)]
    manual_control: bool,
}

impl WebArguments {
    pub(crate) fn settings(self) -> anyhow::Result<hebbian_station::web::WebSettings> {
        Ok(hebbian_station::web::WebSettings {
            recordings_directory: self.recordings_directory,
            enable_recording: self.enable_recording,
            media_address: None,
            media_executable: crate::media::find_media(self.media_executable),
            room: self.room,
            phone_page_url: self.phone_page_url,
            manual_control: self.manual_control,
            media_status: hebbian_station::media_status::MediaStatus::default(),
        })
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

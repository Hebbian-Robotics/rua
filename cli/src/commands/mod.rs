//! The command line: what each command takes, and how its arguments become
//! contract types. `--help` is the agent's manual, so it carries units and examples.
#![allow(
    clippy::doc_markdown,
    reason = "CLI help text is prose for agents, not item documentation."
)]

use std::path::PathBuf;

use clap::{ArgGroup, Args, Parser, Subcommand};
use hebbian_rua::contracts::{Approach, NamedApproach, PoseOrientation, PreviewPose};
use hebbian_rua::{AngleUnit, DEFAULT_SERVER, LengthUnit};

fn default_station_url() -> String {
    std::env::var("HEBBIAN_SERVER").unwrap_or_else(|_| DEFAULT_SERVER.to_owned())
}

/// A minimal `ActionSequenceRequest`, shown in `action-sequence run --help`; a unit test parses it so the
/// help cannot drift from the contract.
macro_rules! action_sequence_request_example {
    () => {
        r#"{"steps": [{"arms": {"arm": {"goal": {"joints_degrees": [0, -30, 30, 0, 0]}, "gripper_opening": 1.0}}}, {"arms": {"arm": {"gripper_opening": 0.0}}}]}"#
    };
}

const ABOUT: &str = "Drive a Hebbian robot-use station from bash.

Commands print one JSON value on stdout and exit. The phone-invite command prints its URL
as plain text unless you pass --raw.

Motion flags name their units. Use millimetres for world positions (+X forward, +Y left,
+Z up) and degrees for joint angles and jaw yaw. Gripper opening is 0 (closed) to 1 (open).
Motion JSON uses the SDK's model fields (position_mm, joints_degrees,
max_joint_speed_degrees_per_second). Use --input-station for native station JSON.
Output is the same concise model view the SDKs give their tools: positions in millimetres
(`position_mm`), joint angles in degrees (`joints_degrees`), rounded, null fields omitted, and
history and diagnostics left out (checkpoints, trace metadata, render geometry, a sequence's full
steps, each arm's last command). `state` also keeps the robot `description` (frames) and the lease
holder. Pass `--raw` for full records with the station's native field units.
Motion commands wait for the outcome and exit 1 unless it completed.

Lease: motion commands (`rehome`, `move-joints`, `move-pose`, `move-arms`, `gripper`, `action-sequence
run`) acquire and renew the lease for the invocation; it is released when the command exits
and expires if the process stops renewing it. Power (`enable`, `disable`, `clear-fault`) is separate and needs no lease of its own.
While another holder (an agent, the viewer's manual controls) holds the lease, commands are
refused with exit 4 and the holder in the error; `stop` is always accepted and `lease revoke`
ends the holder's lease. Motion is refused while an arm has an interlock (see
`state`); `stop`, `disable` and `clear-fault` are still accepted.

Exit codes: 0 completed, 1 refused or not completed (stopped, faulted, limited...), 2 usage,
3 connection failure or timeout, 4 lease held elsewhere.
Failures print {\"error\": {\"kind\", \"detail\", ...}} as JSON on stderr.";

#[derive(Debug, Parser)]
#[command(name = "hebbian-rua", version, about = ABOUT)]
pub struct Cli {
    /// The station's HTTP server.
    #[arg(long, global = true, env = "HEBBIAN_STATION_URL", default_value_t = default_station_url(), value_name = "URL")]
    pub server: String,
    /// Direct media URL for cameras. Defaults to the station URL.
    #[arg(long, global = true, env = "HEBBIAN_MEDIA_SERVER", value_name = "URL")]
    pub media_server: Option<String>,
    /// Print the station's full records in their native units instead of the compact
    /// millimetre and degree form.
    #[arg(long, global = true)]
    pub raw: bool,
    /// Refuse a power or motion command as stale once an arm's revision has moved on, so
    /// nothing acts on an interrupted view (an interlock raised since you read `state`).
    /// `ARM=REVISION` from state's `revision`; repeatable. Not applied to `stop`.
    #[arg(long, global = true, value_name = "ARM=REVISION", value_parser = parse_expected_revision)]
    pub expected_revision: Vec<(String, u64)>,
    /// Take the lease from its current holder, stopping their arms, instead of being refused
    /// with exit 4. Only with the operator's authorization; `lease revoke` is the other override.
    #[arg(long, global = true)]
    pub take_lease: bool,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Print one state snapshot: arms, measured joints, tool pose, faults, interlocks, lease holder.
    ///
    /// Example: hebbian-rua state
    State,
    /// Preflight, then enable the arm holding its current pose.
    ///
    /// Example: hebbian-rua enable arm
    Enable { arm: String },
    /// Approve recovery after inspecting and supporting the arm, once it has faulted or been
    /// disabled and the station is waiting for approval (`recovery_issue` in state).
    ///
    /// Example: hebbian-rua approve-recovery arm
    ApproveRecovery { arm: String },
    /// Turn torque off where the arm is; an unsupported arm falls.
    ///
    /// Example: hebbian-rua disable arm
    Disable { arm: String },
    /// Stop the active motion and hold. Always accepted, whoever holds the lease.
    ///
    /// Example: hebbian-rua stop arm
    Stop { arm: String },
    /// Acknowledge a fault after inspecting the arm.
    ///
    /// Example: hebbian-rua clear-fault arm
    ClearFault { arm: String },
    /// Return to the pose the arm was enabled at.
    ///
    /// Example: hebbian-rua rehome arm
    Rehome { arm: String },
    /// Move to joint angles in the state's joint_names order.
    ///
    /// Example: hebbian-rua move-joints arm --joints-degrees 17,-23,34,6,0 --speed-degrees-per-second 30
    MoveJoints {
        arm: String,
        #[command(flatten)]
        joints: JointArgs,
        #[command(flatten)]
        trajectory: TrajectoryArgs,
    },
    /// Move the tool to a position in the world frame. Without an orientation option the tool
    /// keeps the orientation it has now.
    ///
    /// Example: hebbian-rua move-pose arm --position-mm 200,0,100 --approach down --jaw-yaw-degrees 0
    #[command(group(ArgGroup::new("position").required(true).args(["position_mm", "position_m"])))]
    MovePose {
        arm: String,
        #[command(flatten)]
        pose: PoseArgs,
        #[command(flatten)]
        trajectory: TrajectoryArgs,
    },
    /// Move several arms together: they start and arrive together. Every target is planned first;
    /// one rejection means no arm moves, and if one arm stops the others stop too. Takes a JSON
    /// motion request in SDK model units from a file, or `-` for stdin.
    ///
    /// Example: echo '{"goals":{"left":{"position_mm":[200,150,80],"orientation":{"quaternion":[0,0,0,1]}}},"max_joint_speed_degrees_per_second":30}' | hebbian-rua move-arms -
    MoveArms {
        #[command(flatten)]
        input: MotionFileArgs,
    },
    /// Open or close the gripper: 0 closed, 1 fully open. It stops on an object and keeps
    /// squeezing at the profile's force cap; the outcome says whether it arrived or stopped short.
    ///
    /// Example: hebbian-rua gripper arm 0.5
    Gripper {
        arm: String,
        /// 0 closed to 1 open.
        opening: f64,
    },
    /// Where an arm would take the tool toward a pose; nothing moves. Without an orientation
    /// option the solver chooses the orientation.
    ///
    /// Example: hebbian-rua preview-pose arm --position-mm 200,0,100 --approach forward --jaw-yaw-degrees 90
    #[command(group(ArgGroup::new("position").required(true).args(["position_mm", "position_m"])))]
    PreviewPose {
        arm: String,
        #[command(flatten)]
        pose: PoseArgs,
        #[command(flatten)]
        trajectory: TrajectoryArgs,
    },
    /// Where an arm would take the tool toward each of several independent poses; nothing
    /// moves. Takes a JSON array of {"label"?, "position_mm": [x, y, z], "orientation"?} from a
    /// file or `-`, and prints one projection or refusal per pose in the same order.
    ///
    /// Example: echo '[{"position_mm":[200,0,100],"orientation":{"approach":[0,0,-1],"jaw_yaw_degrees":90}}]' | hebbian-rua preview-poses arm -
    PreviewPoses {
        arm: String,
        #[command(flatten)]
        input: MotionFileArgs,
    },
    /// Run, cancel or read an ordered action sequence across one or more arms.
    #[command(subcommand)]
    ActionSequence(ActionSequenceCommand),
    /// Who holds the lease, and the operator's override.
    #[command(subcommand)]
    Lease(LeaseCommand),
    /// Record the station: spans, state and video.
    #[command(subcommand)]
    Record(RecordCommand),
    /// Print an invite URL that streams a phone as camera `phone_<label>`.
    ///
    /// Example: hebbian-rua phone-invite table_left
    PhoneInvite {
        /// Where the phone is placed: lowercase letters, digits and underscores.
        label: String,
    },
    /// List the cameras, or save a JPEG frame.
    ///
    /// For live video, run `lk cloud auth` or set LIVEKIT_URL, LIVEKIT_API_KEY and LIVEKIT_API_SECRET in the
    /// station's environment or .env. Run `rua camera-devices` and start the station with
    /// `rua serve --camera wrist=2 --camera overhead=4`, using your device selectors.
    /// Open Room viewer at https://hebbian-robotics.github.io/livekit-cameras/ and enter
    /// the server_url and token from `curl --fail http://127.0.0.1:8792/media/access-token`.
    ///
    /// If LiveKit Cloud is unavailable, run `livekit-server --dev` and point LIVEKIT_URL at
    /// ws://127.0.0.1:7880, or restart the station with `--local-only-no-remote-cameras`.
    /// See docs/cameras.md, "When LiveKit Cloud is unavailable".
    #[command(subcommand)]
    Camera(CameraCommand),
    /// The controller's per-cycle traces.
    #[command(subcommand)]
    Traces(TracesCommand),
}

/// A motion request in model units or an explicitly selected native station payload.
#[derive(Debug, Args)]
pub struct MotionFileArgs {
    /// A JSON file, or `-` for stdin.
    pub path: PathBuf,
    /// Read JSON in the native station schema, using the units specified by each field.
    #[arg(long)]
    pub input_station: bool,
}

/// A target pose. Orientation is one of `--quaternion` or `--approach` with `--jaw-yaw-degrees`.
#[derive(Debug, Args)]
#[group(skip)]
pub struct PoseArgs {
    /// Tool position in millimetres.
    #[arg(long, value_parser = parse_vector::<3>, allow_hyphen_values = true, value_name = "X,Y,Z")]
    position_mm: Option<[f64; 3]>,
    /// Tool position in metres.
    #[arg(long, value_parser = parse_vector::<3>, allow_hyphen_values = true, value_name = "X,Y,Z")]
    position_m: Option<[f64; 3]>,
    /// Tool orientation as a quaternion.
    #[arg(long, value_parser = parse_vector::<4>, allow_hyphen_values = true, value_name = "X,Y,Z,W", conflicts_with = "approach")]
    quaternion: Option<[f64; 4]>,
    /// The direction the jaws point: `down` (world -Z), `forward` (world +X) or a world vector X,Y,Z.
    #[arg(long, value_parser = parse_approach, requires = "jaw_yaw_degrees", allow_hyphen_values = true)]
    approach: Option<Approach>,
    /// The jaw yaw about the approach, in degrees.
    #[arg(long, requires = "approach", allow_hyphen_values = true)]
    jaw_yaw_degrees: Option<f64>,
}

/// Joint angle inputs normalized to the station's radians.
#[derive(Debug, Args)]
#[group(required = true, multiple = false)]
pub struct JointArgs {
    /// Comma-separated degrees in the state's joint_names order.
    #[arg(long, value_delimiter = ',', allow_hyphen_values = true)]
    joints_degrees: Option<Vec<f64>>,
    /// Comma-separated radians in the state's joint_names order.
    #[arg(long, value_delimiter = ',', allow_hyphen_values = true)]
    joints_radians: Option<Vec<f64>>,
}

impl JointArgs {
    pub fn into_radians(self) -> Vec<f64> {
        self.joints_radians.unwrap_or_else(|| {
            AngleUnit::Degrees.joint_radians(self.joints_degrees.unwrap_or_default())
        })
    }
}

/// Explicit joint-rate units, converted to radians at the CLI boundary.
#[derive(Debug, Args)]
#[group(skip)]
pub struct TrajectoryArgs {
    /// Peak joint speed in degrees per second; the station clamps it.
    #[arg(long, conflicts_with = "speed_radians_per_second")]
    speed_degrees_per_second: Option<f64>,
    /// Peak joint speed in radians per second; the station clamps it.
    #[arg(long)]
    speed_radians_per_second: Option<f64>,
    /// Joint acceleration cap in degrees per second squared.
    #[arg(long, conflicts_with = "acceleration_radians_per_second_squared")]
    acceleration_degrees_per_second_squared: Option<f64>,
    /// Joint acceleration cap in radians per second squared.
    #[arg(long)]
    acceleration_radians_per_second_squared: Option<f64>,
    /// Joint jerk cap in degrees per second cubed.
    #[arg(long, conflicts_with = "jerk_radians_per_second_cubed")]
    jerk_degrees_per_second_cubed: Option<f64>,
    /// Joint jerk cap in radians per second cubed.
    #[arg(long)]
    jerk_radians_per_second_cubed: Option<f64>,
}

impl TrajectoryArgs {
    pub fn speed_radians(&self) -> Option<f64> {
        self.speed_radians_per_second.or_else(|| {
            self.speed_degrees_per_second
                .map(|value| AngleUnit::Degrees.to_radians(value))
        })
    }

    pub fn acceleration_radians(&self) -> Option<f64> {
        self.acceleration_radians_per_second_squared.or_else(|| {
            self.acceleration_degrees_per_second_squared
                .map(|value| AngleUnit::Degrees.to_radians(value))
        })
    }

    pub fn jerk_radians(&self) -> Option<f64> {
        self.jerk_radians_per_second_cubed.or_else(|| {
            self.jerk_degrees_per_second_cubed
                .map(|value| AngleUnit::Degrees.to_radians(value))
        })
    }
}

impl PoseArgs {
    #[expect(
        clippy::expect_used,
        reason = "the pose subcommands require exactly one position"
    )]
    pub fn position_metres(&self) -> [f64; 3] {
        self.position_m
            .or_else(|| {
                self.position_mm
                    .map(|position| LengthUnit::Millimetres.position_metres(position))
            })
            .expect("clap requires a position")
    }

    pub fn orientation(&self) -> Option<PoseOrientation> {
        if let Some(quaternion) = self.quaternion {
            return Some(PoseOrientation::Quaternion { quaternion });
        }
        Some(PoseOrientation::Grasp {
            approach: self.approach?,
            jaw_yaw_radians: AngleUnit::Degrees.to_radians(self.jaw_yaw_degrees?),
        })
    }

    pub fn preview(&self) -> PreviewPose {
        PreviewPose {
            position: self.position_metres(),
            orientation: self.orientation(),
            max_joint_speed: None,
            max_joint_acceleration: None,
            max_joint_jerk: None,
        }
    }
}

/// `ARM=REVISION`, a revision read from `state`.
fn parse_expected_revision(text: &str) -> Result<(String, u64), String> {
    let (arm, revision) = text
        .split_once('=')
        .ok_or_else(|| "expected ARM=REVISION, such as arm=3".to_owned())?;
    let revision = revision
        .trim()
        .parse::<u64>()
        .map_err(|error| format!("revision {revision:?}: {error}"))?;
    Ok((arm.trim().to_owned(), revision))
}

/// Exactly `N` comma-separated numbers.
fn parse_vector<const N: usize>(text: &str) -> Result<[f64; N], String> {
    let parts: Vec<f64> = text
        .split(',')
        .map(|part| {
            part.trim()
                .parse::<f64>()
                .map_err(|error| error.to_string())
        })
        .collect::<Result<_, _>>()?;
    <[f64; N]>::try_from(parts)
        .map_err(|parts| format!("expected {N} numbers, got {}", parts.len()))
}

fn parse_approach(text: &str) -> Result<Approach, String> {
    match text {
        "down" => Ok(Approach::Named(NamedApproach::Down)),
        "forward" => Ok(Approach::Named(NamedApproach::Forward)),
        vector => parse_vector::<3>(vector).map(Approach::Direction),
    }
}
#[derive(Debug, Subcommand)]
pub enum ActionSequenceCommand {
    /// Run an action sequence in SDK model units from a file or `-` for stdin and wait for it to
    /// end; exits 1 unless it completed. Acquires the lease for the run: it dies with the socket.
    ///
    /// Example: hebbian-rua action-sequence run steps.json
    #[command(after_long_help = concat!(
        "A request (steps run in order; arms within a step start together and settle before \
         the next step; goal and gripper targets on an arm start together):\n",
        action_sequence_request_example!(),
        "\nA pose goal is {\"position_mm\": [x, y, z], \"orientation\": ...}, \
         where orientation is {\"quaternion\": [x, y, z, w]} or \
         {\"approach\": [x, y, z], \"jaw_yaw_degrees\": d}.\nFor a timed single-arm move, set at_seconds beside each waypoint's at and the \
         step's goal. Times are in seconds from the start of the step and must increase."
    ))]
    Run {
        #[command(flatten)]
        input: MotionFileArgs,
    },
    /// Cancel a running action sequence by id and print its final record.
    ///
    /// Example: hebbian-rua action-sequence cancel seq-1
    Cancel { id: String },
    /// Print an action sequence's record by id.
    ///
    /// Example: hebbian-rua action-sequence get seq-1
    Get { id: String },
}

#[derive(Debug, Subcommand)]
pub enum LeaseCommand {
    /// Print who holds the lease now, or null.
    ///
    /// Example: hebbian-rua lease status
    Status,
    /// The operator's override: end whoever holds the lease. A running loop's commands are then
    /// refused.
    ///
    /// Example: hebbian-rua lease revoke
    Revoke,
}

#[derive(Debug, Subcommand)]
pub enum RecordCommand {
    /// Start recording until `record stop`; prints the recording's name.
    ///
    /// Example: hebbian-rua record start
    Start,
    /// Request recording stop and print `{"stopped":true}`. Check status for finalization.
    ///
    /// Example: hebbian-rua record stop
    Stop,
    /// Print the recorder state as JSON, with a recording name when available.
    ///
    /// Example: hebbian-rua record status
    Status,
}

#[derive(Debug, Subcommand)]
pub enum CameraCommand {
    /// List the cameras the media service knows, as a JSON array of {name, available}.
    ///
    /// Example: hebbian-rua camera list
    List,
    /// Save the newest JPEG frame of a camera to a file and print its path.
    ///
    /// Example: hebbian-rua camera snapshot wrist --out wrist.jpg
    Snapshot {
        name: String,
        #[arg(long)]
        out: PathBuf,
    },
}

#[derive(Debug, Subcommand)]
pub enum TracesCommand {
    /// List the trace files, newest first.
    ///
    /// Example: hebbian-rua traces list
    List,
    /// Download traces into a directory exactly as served, still gzip-compressed (`gunzip` them);
    /// every trace unless names are given. Prints the paths written.
    ///
    /// Example: hebbian-rua traces pull --output traces
    Pull {
        names: Vec<String>,
        #[arg(long, default_value = "traces")]
        output: PathBuf,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(arguments: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("hebbian-rua").chain(arguments.iter().copied()))
    }

    #[test]
    fn the_action_sequence_example_in_the_help_is_a_valid_request() {
        let request: hebbian_rua::contracts::ActionSequenceRequest = serde_json::from_value(
            hebbian_rua::normalize_robot_tool_arguments(
                serde_json::from_str(action_sequence_request_example!()).expect("valid JSON"),
            )
            .expect("valid model units"),
        )
        .expect("the help example parses");
        assert_eq!(request.steps.len(), 2);
    }

    #[test]
    fn move_pose_takes_negative_numbers_and_a_quaternion() {
        let cli = parse(&[
            "move-pose",
            "left",
            "--position-m",
            "0.2,-0.15,0.08",
            "--quaternion",
            "0,0,0,1",
        ])
        .expect("parses");
        let Command::MovePose { pose, .. } = cli.command else {
            panic!("wrong command");
        };
        assert_eq!(pose.position_metres(), [0.2, -0.15, 0.08]);
        assert_eq!(
            pose.orientation(),
            Some(PoseOrientation::Quaternion {
                quaternion: [0.0, 0.0, 0.0, 1.0]
            })
        );
    }

    #[test]
    fn an_approach_needs_a_jaw_yaw_and_excludes_a_quaternion() {
        assert!(
            parse(&[
                "preview-pose",
                "a",
                "--position-m",
                "0,0,0",
                "--approach",
                "down"
            ])
            .is_err()
        );
        assert!(
            parse(&[
                "preview-pose",
                "a",
                "--position-m",
                "0,0,0",
                "--approach",
                "down",
                "--jaw-yaw-degrees",
                "0",
                "--quaternion",
                "0,0,0,1"
            ])
            .is_err()
        );
        let cli = parse(&[
            "preview-pose",
            "a",
            "--position-m",
            "0,0,0",
            "--approach",
            "0,-1,0",
            "--jaw-yaw-degrees",
            "-45",
        ])
        .expect("parses");
        let Command::PreviewPose { pose, .. } = cli.command else {
            panic!("wrong command");
        };
        assert_eq!(
            pose.orientation(),
            Some(PoseOrientation::Grasp {
                approach: Approach::Direction([0.0, -1.0, 0.0]),
                jaw_yaw_radians: -std::f64::consts::FRAC_PI_4
            })
        );
    }

    #[test]
    fn motion_flags_require_explicit_and_unambiguous_units() {
        for arguments in [
            &["move-pose", "arm", "--position", "0.2,0,0.1"][..],
            &["move-pose", "arm", "--position-mm", "200,0"],
            &[
                "move-pose",
                "arm",
                "--position-mm",
                "200,0,100",
                "--position-m",
                "0.2,0,0.1",
            ],
            &["move-joints", "arm", "--joints", "0,0,0"],
            &[
                "move-joints",
                "arm",
                "--joints-degrees",
                "0,0,0",
                "--joints-radians",
                "0,0,0",
            ],
            &[
                "preview-pose",
                "arm",
                "--position-mm",
                "200,0,100",
                "--speed-degrees-per-second",
                "30",
                "--speed-radians-per-second",
                "0.5",
            ],
        ] {
            assert!(parse(arguments).is_err(), "{arguments:?}");
        }
    }

    #[test]
    fn a_millimetre_preview_converts_position_and_joint_caps() {
        let cli = parse(&[
            "preview-pose",
            "arm",
            "--position-mm",
            "200,-150,80",
            "--speed-degrees-per-second",
            "30",
            "--acceleration-degrees-per-second-squared",
            "90",
            "--jerk-degrees-per-second-cubed",
            "180",
        ])
        .expect("the model-facing flags parse");
        let Command::PreviewPose {
            pose, trajectory, ..
        } = cli.command
        else {
            panic!("wrong command");
        };
        assert_eq!(pose.preview().position, [0.2, -0.15, 0.08]);
        let speed = trajectory.speed_radians().expect("requested speed");
        assert!((speed - std::f64::consts::FRAC_PI_6).abs() < 1e-12);
        assert_eq!(
            trajectory.acceleration_radians(),
            Some(std::f64::consts::FRAC_PI_2)
        );
        assert_eq!(trajectory.jerk_radians(), Some(std::f64::consts::PI));
    }

    #[test]
    fn expected_revisions_take_an_arm_and_a_revision() {
        let cli = parse(&[
            "--expected-revision",
            "follower=3",
            "--expected-revision",
            "left=0",
            "state",
        ])
        .unwrap();
        assert_eq!(
            cli.expected_revision,
            [("follower".to_owned(), 3), ("left".to_owned(), 0)]
        );
        assert!(parse(&["--expected-revision", "follower", "state"]).is_err());
        assert!(parse(&["--expected-revision", "follower=-1", "state"]).is_err());
    }

    #[test]
    fn joints_and_gripper_parse() {
        let cli = parse(&[
            "move-joints",
            "a",
            "--joints-radians",
            "0.1,-0.2,0.3",
            "--speed-radians-per-second",
            "0.5",
        ])
        .expect("parses");
        let Command::MoveJoints {
            joints, trajectory, ..
        } = cli.command
        else {
            panic!("wrong command");
        };
        assert_eq!(joints.into_radians(), [0.1, -0.2, 0.3]);
        assert_eq!(trajectory.speed_radians(), Some(0.5));
        assert!(parse(&["gripper", "a", "0.5"]).is_ok());
    }

    #[test]
    fn documented_auxiliary_commands_accept_their_public_arguments() {
        for arguments in [
            &["record", "start"][..],
            &["record", "status"],
            &["phone-invite", "table_left"],
            &["camera", "list"],
            &["camera", "snapshot", "wrist", "--out", "wrist.jpg"],
            &["traces", "list"],
            &["traces", "pull", "a.gz", "b.gz", "--output", "out"],
            &["action-sequence", "cancel", "seq-1"],
        ] {
            parse(arguments).unwrap_or_else(|error| panic!("{arguments:?}: {error}"));
        }
    }

    #[test]
    fn camera_snapshot_requires_an_output_path() {
        let error = parse(&["camera", "snapshot", "wrist"])
            .expect_err("omitting the snapshot destination must fail");
        assert_eq!(
            error.kind(),
            clap::error::ErrorKind::MissingRequiredArgument
        );
        assert!(error.to_string().contains("--out"));
    }
}

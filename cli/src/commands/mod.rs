//! The command line: what each command takes, and how its arguments become
//! contract types. `--help` is the agent's manual, so it carries units and examples.
#![allow(
    clippy::doc_markdown,
    reason = "CLI help text is prose for agents, not item documentation."
)]

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};
use hebbian_robot_use_sdk::DEFAULT_SERVER;
use hebbian_robot_use_sdk::contracts::{Approach, NamedApproach, PoseOrientation, PreviewPose};

fn default_station_url() -> String {
    std::env::var("HEBBIAN_SERVER").unwrap_or_else(|_| DEFAULT_SERVER.to_owned())
}

/// A minimal `ActionSequenceRequest`, shown in `action-sequence run --help`; a unit test parses it so the
/// help cannot drift from the contract.
macro_rules! action_sequence_request_example {
    () => {
        r#"{"steps": [{"arms": {"arm": {"movement": {"kind": "joints", "joints": [0.0, -0.5, 0.5, 0.0, 0.0]}, "gripper_opening": 1.0}}}, {"arms": {"arm": {"gripper_opening": 0.0}}}]}"#
    };
}

const ABOUT: &str = "Drive a Hebbian robot-use station from bash.

Every command opens one connection, does its job, prints exactly one JSON value on stdout
and exits. Nothing stays running and nothing is prompted.

Input units: joint angles in radians, positions in metres in the robot's world frame (x forward,
z up), speeds in rad/s (the station clamps them), grippers 0 (closed) to 1 (open), jaw yaw in degrees.
Output is the same concise model view the SDKs give their tools: positions in millimetres
(`position_mm`), joint angles in degrees (`joints_degrees`), rounded, null fields omitted, and
history and diagnostics left out (checkpoints, trace metadata, render geometry, a sequence's full
steps, each arm's last command). `state` also keeps the robot `description` (frames) and the lease
holder. Pass `--raw` for the station's full records in SI units (metres and radians).
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
#[command(name = "hebbian-robot-use", version, about = ABOUT)]
pub struct Cli {
    /// The station's HTTP server.
    #[arg(long, global = true, env = "HEBBIAN_STATION_URL", default_value_t = default_station_url(), value_name = "URL")]
    pub server: String,
    /// Direct media URL for cameras. Defaults to the station URL.
    #[arg(long, global = true, env = "HEBBIAN_MEDIA_SERVER", value_name = "URL")]
    pub media_server: Option<String>,
    /// Print the station's full records in SI units (metres, radians) instead of the compact
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
    /// Example: hebbian-robot-use state
    State,
    /// Preflight, then enable the arm holding its current pose.
    ///
    /// Example: hebbian-robot-use enable arm
    Enable { arm: String },
    /// Approve recovery after inspecting and supporting the arm, once it has faulted or been
    /// disabled and the station is waiting for approval (`recovery_issue` in state).
    ///
    /// Example: hebbian-robot-use approve-recovery arm
    ApproveRecovery { arm: String },
    /// Turn torque off where the arm is; an unsupported arm falls.
    ///
    /// Example: hebbian-robot-use disable arm
    Disable { arm: String },
    /// Stop the active motion and hold. Always accepted, whoever holds the lease.
    ///
    /// Example: hebbian-robot-use stop arm
    Stop { arm: String },
    /// Acknowledge a fault after inspecting the arm.
    ///
    /// Example: hebbian-robot-use clear-fault arm
    ClearFault { arm: String },
    /// Return to the pose the arm was enabled at, then close the gripper.
    ///
    /// Example: hebbian-robot-use rehome arm
    Rehome { arm: String },
    /// Move to joint angles in radians, one per joint in the state's `joint_names` order.
    ///
    /// Example: hebbian-robot-use move-joints arm --joints 0.3,-0.4,0.6,0.1,0.0 --speed 0.5
    MoveJoints {
        arm: String,
        /// Comma-separated radians.
        #[arg(
            long,
            value_delimiter = ',',
            required = true,
            allow_hyphen_values = true
        )]
        joints: Vec<f64>,
        /// Requested peak joint speed in rad/s; the station clamps it.
        #[arg(long)]
        speed: Option<f64>,
    },
    /// Move the tool to a position in metres. Without an orientation option the tool keeps the
    /// orientation it has now.
    ///
    /// Example: hebbian-robot-use move-pose arm --position 0.2,0.0,0.1 --approach down --jaw-yaw-degrees 0
    MovePose {
        arm: String,
        #[command(flatten)]
        pose: PoseArgs,
        /// Requested peak joint speed in rad/s; the station clamps it.
        #[arg(long)]
        speed: Option<f64>,
    },
    /// Move several arms together: they start and arrive together. Every target is planned first;
    /// one rejection means no arm moves, and if one arm stops the others stop too. Takes a JSON
    /// MoveArmsCommand from a file, or `-` for stdin.
    ///
    /// Example: echo '{"targets":{"left":{"kind":"pose","position":[0.2,0.15,0.08],"orientation":{"kind":"quaternion","quaternion":[0,0,0,1]}}},"max_joint_speed":0.5}' | hebbian-robot-use move-arms -
    MoveArms {
        /// A JSON file, or `-` for stdin.
        input: PathBuf,
    },
    /// Open or close the gripper: 0 closed, 1 fully open. It stops on an object and keeps
    /// squeezing at the profile's force cap; the outcome says whether it arrived or stopped short.
    ///
    /// Example: hebbian-robot-use gripper arm 0.5
    Gripper {
        arm: String,
        /// 0 closed to 1 open.
        opening: f64,
    },
    /// Where an arm would take the tool toward a pose; nothing moves. Without an orientation
    /// option the solver chooses the orientation.
    ///
    /// Example: hebbian-robot-use preview-pose arm --position 0.2,0.0,0.1 --approach forward --jaw-yaw-degrees 90
    PreviewPose {
        arm: String,
        #[command(flatten)]
        pose: PoseArgs,
    },
    /// Where an arm would take the tool toward each of several independent poses; nothing
    /// moves. Takes a JSON array of {"label"?, "position": [x, y, z], "orientation"?} from a
    /// file or `-`, and prints one projection or refusal per pose in the same order.
    ///
    /// Example: echo '[{"position":[0.2,0.0,0.1],"orientation":{"kind":"grasp","approach":[0,0,-1],"jaw_yaw_degrees":90}}]' | hebbian-robot-use preview-poses arm -
    PreviewPoses {
        arm: String,
        /// A JSON file, or `-` for stdin.
        input: PathBuf,
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
    /// Example: hebbian-robot-use phone-invite table_left
    PhoneInvite {
        /// Where the phone is placed: lowercase letters, digits and underscores.
        label: String,
    },
    /// List the cameras, or save a JPEG frame.
    #[command(subcommand)]
    Camera(CameraCommand),
    /// The controller's per-cycle traces.
    #[command(subcommand)]
    Traces(TracesCommand),
}

/// A target pose. Orientation is one of `--quaternion` or `--approach` with `--jaw-yaw-degrees`.
#[derive(Debug, Args)]
pub struct PoseArgs {
    /// Tool position in metres.
    #[arg(long, value_parser = parse_vector::<3>, allow_hyphen_values = true, required = true, value_name = "X,Y,Z")]
    position: [f64; 3],
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

impl PoseArgs {
    pub fn position(&self) -> [f64; 3] {
        self.position
    }

    pub fn orientation(&self) -> Option<PoseOrientation> {
        if let Some(quaternion) = self.quaternion {
            return Some(PoseOrientation::Quaternion { quaternion });
        }
        Some(PoseOrientation::Grasp {
            approach: self.approach?,
            jaw_yaw_degrees: self.jaw_yaw_degrees?,
        })
    }

    pub fn preview(&self) -> PreviewPose {
        PreviewPose {
            position: self.position,
            orientation: self.orientation(),
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
    /// Run a JSON ActionSequenceRequest from a file or `-` for stdin and wait for it to
    /// end; exits 1 unless it completed. Acquires the lease for the run: it dies with the socket.
    ///
    /// Example: hebbian-robot-use action-sequence run steps.json
    #[command(after_long_help = concat!(
        "A request (steps run in order; arms within a step start together and settle before \
         the next step; movement and gripper targets on an arm start together):\n",
        action_sequence_request_example!(),
        "\nA pose movement is {\"kind\": \"pose\", \"position\": [x, y, z], \"orientation\": ...}, \
         where orientation is {\"kind\": \"quaternion\", \"quaternion\": [x, y, z, w]} or \
         {\"kind\": \"grasp\", \"approach\": [x, y, z], \"jaw_yaw_degrees\": d}."
    ))]
    Run {
        /// A JSON file, or `-` for stdin.
        input: PathBuf,
    },
    /// Cancel a running action sequence by id and print its final record.
    ///
    /// Example: hebbian-robot-use action-sequence cancel seq-1
    Cancel { id: String },
    /// Print an action sequence's record by id.
    ///
    /// Example: hebbian-robot-use action-sequence get seq-1
    Get { id: String },
}

#[derive(Debug, Subcommand)]
pub enum LeaseCommand {
    /// Print who holds the lease now, or null.
    ///
    /// Example: hebbian-robot-use lease status
    Status,
    /// The operator's override: end whoever holds the lease. A running loop's commands are then
    /// refused.
    ///
    /// Example: hebbian-robot-use lease revoke
    Revoke,
}

#[derive(Debug, Subcommand)]
pub enum RecordCommand {
    /// Start recording until `record stop`; prints the recording's name.
    ///
    /// Example: hebbian-robot-use record start
    Start,
    /// Stop recording.
    ///
    /// Example: hebbian-robot-use record stop
    Stop,
    /// Print the recording status.
    ///
    /// Example: hebbian-robot-use record status
    Status,
}

#[derive(Debug, Subcommand)]
pub enum CameraCommand {
    /// List the cameras the media service knows, as a JSON array of {name, available}.
    ///
    /// Example: hebbian-robot-use camera list
    List,
    /// Save the newest JPEG frame of a camera to a file and print its path.
    ///
    /// Example: hebbian-robot-use camera snapshot wrist --out wrist.jpg
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
    /// Example: hebbian-robot-use traces list
    List,
    /// Download traces into a directory exactly as served, still gzip-compressed (`gunzip` them);
    /// every trace unless names are given. Prints the paths written.
    ///
    /// Example: hebbian-robot-use traces pull --output traces
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
        Cli::try_parse_from(std::iter::once("hebbian-robot-use").chain(arguments.iter().copied()))
    }

    #[test]
    fn the_action_sequence_example_in_the_help_is_a_valid_request() {
        let request: hebbian_robot_use_sdk::contracts::ActionSequenceRequest =
            serde_json::from_str(action_sequence_request_example!())
                .expect("the help example parses");
        assert_eq!(request.steps.len(), 2);
    }

    #[test]
    fn defaults_match_the_station() {
        let cli = parse(&["state"]).expect("state parses");
        assert_eq!(cli.server, DEFAULT_SERVER);
        assert!(cli.media_server.is_none());
    }

    #[test]
    fn move_pose_takes_negative_numbers_and_a_quaternion() {
        let cli = parse(&[
            "move-pose",
            "left",
            "--position",
            "0.2,-0.15,0.08",
            "--quaternion",
            "0,0,0,1",
        ])
        .expect("parses");
        let Command::MovePose { pose, .. } = cli.command else {
            panic!("wrong command");
        };
        assert_eq!(pose.position(), [0.2, -0.15, 0.08]);
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
                "--position",
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
                "--position",
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
            "--position",
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
                jaw_yaw_degrees: -45.0
            })
        );
    }

    #[test]
    fn positions_need_three_numbers() {
        assert!(parse(&["move-pose", "a", "--position", "1,2"]).is_err());
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
            "--joints",
            "0.1,-0.2,0.3",
            "--speed",
            "0.5",
        ])
        .expect("parses");
        let Command::MoveJoints { joints, speed, .. } = cli.command else {
            panic!("wrong command");
        };
        assert_eq!(joints, [0.1, -0.2, 0.3]);
        assert_eq!(speed, Some(0.5));
        assert!(parse(&["gripper", "a", "0.5"]).is_ok());
    }

    #[test]
    fn recording_camera_phone_and_traces_commands_parse() {
        for arguments in [
            &["record", "start"][..],
            &["record", "status"],
            &["phone-invite", "table_left"],
            &["camera", "list"],
            &["camera", "snapshot", "wrist", "--out", "wrist.jpg"],
            &["traces", "list"],
            &["traces", "pull", "a.gz", "b.gz", "--output", "out"],
            &["action-sequence", "cancel", "seq-1"],
            &["lease", "revoke"],
        ] {
            parse(arguments).unwrap_or_else(|error| panic!("{arguments:?}: {error}"));
        }
        assert!(parse(&["camera", "snapshot", "wrist"]).is_err());
    }
}

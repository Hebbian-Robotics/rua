//! Arm feedback frames and motion inputs shared by the station and controller.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    ArmMode, ArmRecoveryIssue, CommandRecord, CornerPlan, IkResult, JointSpeed, LeaseAnswer,
    PathLimited, PlannedMotion, PoseDescription, PoseLimited, PoseProjection, RejectionReason,
    UnixNanos,
};

/// One arm's part of a grouped move: a single straight run from where the arm
/// holds to its target.
#[derive(Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ArmMove {
    pub arm: String,
    pub plan: CornerPlan,
    /// The speed the plan was made at, after clamping; recorded.
    pub joint_speed: JointSpeed,
    pub path_limited: Option<PathLimited>,
    pub pose_limited: Option<PoseLimited>,
}

/// One control cycle's state; a sample, so a slow client may miss some.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Sample {
    pub cycle: u64,
    pub mode: ArmMode,
    /// The command being carried out, when one is.
    pub active_command: Option<String>,
    /// The segment of the active motion being followed.
    pub segment: Option<String>,
    /// Where the arm holds between commands, which a plan must start from; only
    /// while the motors are enabled.
    pub hold_pose: Option<Vec<f64>>,
    /// Where the arm rested unpowered when it was enabled; shutdown returns there.
    pub rest_pose: Option<Vec<f64>>,
    pub positions: Vec<f64>,
    /// Joint setpoint successfully sent this cycle, if any.
    pub commanded_positions: Option<Vec<f64>>,
    pub velocities: Vec<f64>,
    /// Each arm joint's load, in percent of the servo's rated load.
    pub efforts: Vec<f64>,
    /// The last reading of each driven motor, the gripper's last; empty in
    /// simulation. Sent while tripped too, so a holding arm that heats shows.
    pub temperatures_celsius: Vec<f64>,
    /// The supply voltage each driven motor last reported, as the temperatures.
    pub voltages_volts: Vec<f64>,
    pub gripper_angle: Option<f64>,
    /// A fault the hardware reports this cycle, tripped or not.
    pub fault: Option<String>,
    /// The protective check currently preventing new motion, if any.
    #[serde(default)]
    pub active_trip: Option<String>,
    pub recovery_issue: Option<ArmRecoveryIssue>,
    pub cycle_seconds: f64,
}

impl Sample {
    /// An empty sample, for the loop to fill.
    #[must_use]
    pub fn blank() -> Self {
        Self {
            cycle: 0,
            mode: ArmMode::Disabled,
            active_command: None,
            segment: None,
            hold_pose: None,
            rest_pose: None,
            positions: Vec::new(),
            commanded_positions: None,
            velocities: Vec::new(),
            efforts: Vec::new(),
            temperatures_celsius: Vec::new(),
            voltages_volts: Vec::new(),
            gripper_angle: None,
            fault: None,
            active_trip: None,
            recovery_issue: None,
            cycle_seconds: 0.0,
        }
    }
}

/// One line from the controller: a log event with its sequence number, or an
/// unsequenced sample or reply.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Frame {
    /// Present on log events only; numbers rise by one with no gaps.
    pub seq: Option<u64>,
    /// When the controller's writer made the frame; a replayed log event keeps
    /// the time it was first emitted.
    pub emitted_unix_nano: UnixNanos,
    pub event: Event,
}

impl Frame {
    /// A frame emitted now.
    #[must_use]
    pub fn emitted_now(seq: Option<u64>, event: Event) -> Self {
        Self {
            seq,
            emitted_unix_nano: UnixNanos::now(),
            event,
        }
    }
}

/// Controller to client.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Event {
    #[schemars(title = "State")]
    State(Sample),
    /// The one answer to a request that carried a request id: the command's
    /// record when it was accepted, or the reason it was not.
    #[schemars(title = "Reply")]
    Reply {
        request_id: String,
        accepted: bool,
        record: Option<CommandRecord>,
        /// The records of a grouped move, one per arm in the order asked; empty
        /// for any other request.
        #[serde(default)]
        records: Vec<CommandRecord>,
        /// The plan a `plan_motion` made.
        plan: Option<Box<PlannedMotion>>,
        /// The answer to a `project_pose`.
        projection: Option<Box<PoseProjection>>,
        /// The answer to a `describe_pose`.
        description: Option<Box<PoseDescription>>,
        /// How a `solve_pose` ended.
        ik: Option<Box<IkResult>>,
        /// The answer to a request about the lease.
        lease: Option<Box<LeaseAnswer>>,
        reason: Option<RejectionReason>,
        detail: Option<String>,
    },
    /// The answer to `snapshot`, and to `subscribe` when events were lost.
    #[schemars(title = "SnapshotTaken")]
    Snapshot {
        /// The newest sequence number issued; subscribing after it misses nothing.
        seq: u64,
        sample: Option<Sample>,
        mode: ArmMode,
        fault: Option<String>,
        /// The commands the arm still remembers, oldest first, as they stand.
        records: Vec<CommandRecord>,
        /// The retained log, oldest first.
        log: Vec<Frame>,
    },
    /// A resume point the retained log no longer reaches, or that is ahead of
    /// it: events between `after` and `oldest_retained` are lost. A snapshot
    /// follows.
    #[schemars(title = "Resync")]
    Resync {
        after: u64,
        oldest_retained: u64,
        newest: u64,
    },
    /// A command's record changed: accepted, started, a checkpoint, ended.
    #[schemars(title = "Record")]
    Record(CommandRecord),
    /// The arm changed mode.
    #[schemars(title = "Mode")]
    Mode {
        mode: ArmMode,
        /// Why, after a fault.
        fault: Option<String>,
    },
    /// A request that was not acted on, when it carried no request id to reply to.
    #[schemars(title = "Rejected")]
    Rejected { reason: String },
    /// A safety check stopped the arm, which holds where it is. Reported once
    /// each time a check starts failing.
    #[schemars(title = "Tripped")]
    Tripped { reason: String },
    /// The station lost its motor-controller process; the arm's torque is unknown.
    ControllerUnavailable { reason: String },
    /// A fresh controller passed startup checks and the arm is disabled.
    ControllerAvailable,
}

impl Event {
    /// Whether this goes in the sequenced log. Samples and per-client replies do
    /// not: a sample is superseded by the next, a reply answers one client.
    #[must_use]
    pub fn is_logged(&self) -> bool {
        !matches!(
            self,
            Self::State(_) | Self::Reply { .. } | Self::Snapshot { .. }
        )
    }
}

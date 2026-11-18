//! Messages between clients and the executor: one JSON object per line over
//! the in-process link. These types are the contract:
//! `hebbian-robot-use-server schema` writes their JSON Schema, and the Python and
//! TypeScript types are generated from it.
//!
//! A client sends an [`Envelope`] (a request, and optionally a request id). The
//! executor answers with [`Frame`]s: samples, which may be dropped, and log
//! events, which carry a station-wide sequence number and are never dropped, so a
//! client that reconnects can resume from the last number it saw.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    ArmMode, ArmRecoveryIssue, Command, CommandRecord, CornerPlan, IkResult, LeaseAnswer,
    LeaseHolder, OnInterruption, OrientationGoal, PathLimited, PlannedMotion, PoseDescription,
    PoseLimited, PoseOrientation, PoseProjection, RejectionReason, UnixNanos, Waypoint,
};

/// Client to executor.
#[derive(Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Request {
    /// A command for the arm: admitted into its state machine and answered with
    /// the command's record, or refused with a reason.
    #[schemars(title = "CommandRequest")]
    Command {
        command: Command,
        /// The lease token the client holds; none when it holds none.
        lease_token: Option<String>,
    },
    /// Move several arms of this executor together: one shared trajectory and
    /// tracking gate across their joints, in the order given. Every arm admits
    /// its part or none does; the reply carries one record per arm, in order.
    /// When one arm's part ends without completing, the others stop and hold.
    #[schemars(title = "MoveArmsRequest")]
    MoveArms {
        /// What the client calls this kind of move (`move_arms`).
        label: String,
        moves: Vec<ArmMove>,
        /// The lease token the client holds; none when it holds none.
        lease_token: Option<String>,
    },
    /// Acquire the lease on every arm of this executor: granted when no one
    /// holds it, else answered with the holder.
    #[schemars(title = "AcquireLeaseRequest")]
    AcquireLease {
        holder: LeaseHolder,
        label: String,
        required_cameras: Vec<String>,
        on_interruption: OnInterruption,
    },
    /// Take the lease from whoever holds it, stopping every arm first.
    #[schemars(title = "TakeLeaseRequest")]
    TakeLease {
        holder: LeaseHolder,
        label: String,
        required_cameras: Vec<String>,
        on_interruption: OnInterruption,
    },
    /// Keep the lease from lapsing; refused when the token is not the holder's.
    #[schemars(title = "RetainLeaseRequest")]
    RetainLease { lease_token: Option<String> },
    /// Give the lease up; does nothing for a token that is not the holder's.
    #[schemars(title = "ReturnLeaseRequest")]
    ReturnLease { lease_token: Option<String> },
    /// The operator's override: end whoever holds the lease.
    #[schemars(title = "RevokeLeaseRequest")]
    RevokeLease,
    /// Ask who holds the lease; the reply carries the holder, if any.
    #[schemars(title = "ReadLeaseRequest")]
    ReadLease,
    /// Plan a move from `start` through the waypoints, without moving the arm:
    /// each pose solved, every corner checked against the joint limits, and every
    /// straight joint line against self-collision and the table. The reply carries
    /// the plan to send as a `move`, or the reason it was refused.
    #[schemars(title = "PlanMotionRequest")]
    PlanMotion {
        /// Where the arm holds, which the plan starts from.
        start: Vec<f64>,
        waypoints: Vec<Waypoint>,
        /// The speed asked for; none for the arm's default. Never above its limit.
        joint_speed: Option<f64>,
        /// Measured gripper angle used by the trajectory's geometry checks.
        #[serde(default)]
        gripper_angle: Option<f64>,
    },
    /// Where the arm could take the tool toward a pose from the measured joints,
    /// without moving it: for a policy arm the policy's own rehearsal, otherwise
    /// the solve and the checks a move would get. An orientation left out is the
    /// solver's to choose. The reply carries the projection.
    #[schemars(title = "ProjectPoseRequest")]
    ProjectPose {
        position: [f64; 3],
        orientation: Option<PoseOrientation>,
        /// The arm's measured joints.
        joints: Vec<f64>,
        /// The jaw's angle, when the arm has a gripper.
        gripper_angle: Option<f64>,
    },
    /// The tool's pose at these joints, in the station's world frame.
    #[schemars(title = "DescribePoseRequest")]
    DescribePose { joints: Vec<f64> },
    /// Solve inverse kinematics for the arm's tool, without moving the arm:
    /// joints that put the tool at `position` with `orientation`, searched from
    /// `seed`. The reply carries the solve's result.
    #[schemars(title = "SolvePoseRequest")]
    SolvePose {
        /// Where the tool goes, in the station's world frame.
        position: [f64; 3],
        orientation: OrientationGoal,
        /// The arm's joints the search starts from, one per arm joint.
        seed: Vec<f64>,
    },
    /// Ask for a snapshot: the log's head, the latest sample and the retained
    /// log, in one message.
    #[schemars(title = "SnapshotRequest")]
    Snapshot,
    /// Replay the retained log after sequence number `after` (all of it when
    /// absent), then continue live. When `after` is older than the retained log,
    /// a snapshot comes first, since events were lost to retention.
    #[schemars(title = "Subscribe")]
    Subscribe {
        #[serde(default)]
        after: Option<u64>,
    },
}

/// One arm's part of a grouped move: a single straight run from where the arm
/// holds to its target.
#[derive(Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ArmMove {
    pub arm: String,
    pub plan: CornerPlan,
    /// The speed the plan was made at, after clamping; recorded.
    pub joint_speed: f64,
    pub path_limited: Option<PathLimited>,
    pub pose_limited: Option<PoseLimited>,
}

/// A request and the id the client chose for it. Every request with an id gets
/// exactly one [`Event::Reply`]; sending an id again repeats that reply without
/// acting twice.
#[derive(Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Envelope {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    pub request: Request,
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

/// One line from the executor: a log event with its sequence number, or an
/// unsequenced sample or reply.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Frame {
    /// Present on log events only; numbers rise by one with no gaps.
    pub seq: Option<u64>,
    /// When the executor's writer made the frame; a replayed log event keeps
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

/// Executor to client.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_needs_no_request_id() {
        let parsed = serde_json::from_str::<Envelope>(
            r#"{"request":{"kind":"command","command":{"kind":"stop"}}}"#,
        )
        .unwrap();
        assert_eq!(parsed.request_id, None);
        assert_eq!(
            parsed.request,
            Request::Command {
                command: Command::Stop,
                lease_token: None,
            }
        );
    }

    #[test]
    fn an_unknown_command_is_refused() {
        assert!(
            serde_json::from_str::<Envelope>(
                r#"{"request":{"kind":"command","command":{"kind":"fly"}}}"#
            )
            .is_err()
        );
    }
}

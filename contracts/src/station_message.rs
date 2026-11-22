//! Shared request and event models used by the station and generated SDKs.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    ActionSequenceRecord, ActionSequenceRequest, CommandAnswer, Frame, LeaseAnswer, LeaseHolder,
    LiveRun, MoveArmsCommand, OnInterruption, PreviewAnswer, PreviewPose, RunEvent, StationCommand,
    StationState, TraceContext, TraceSpan, UnixNanos,
};

/// Shared client request models used by SDK command and telemetry helpers.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ClientMessage {
    /// Submit an ordered action sequence under this connection's lease.
    #[schemars(title = "StartActionSequence")]
    StartActionSequence {
        request_id: Option<String>,
        action_sequence: ActionSequenceRequest,
    },
    /// Cancel an action sequence by its identifier.
    #[schemars(title = "CancelActionSequence")]
    CancelActionSequence {
        request_id: Option<String>,
        action_sequence_id: String,
    },
    /// The first message: the last sequence number seen per arm, for the arms
    /// to resume; the others start from a snapshot.
    #[schemars(title = "ClientHello")]
    Hello {
        #[serde(default)]
        resume_after: BTreeMap<String, u64>,
        /// The last run sequence number seen; without one, the run telemetry
        /// stream starts at its newest record.
        #[serde(default)]
        run_after: Option<u64>,
    },
    /// Acquire the lease on every arm for this connection, answered by
    /// `lease`: granted when no one holds it, else the holder.
    #[schemars(title = "AcquireLease")]
    AcquireLease {
        #[serde(default)]
        request_id: Option<String>,
        holder: LeaseHolder,
        label: String,
        /// Cameras the holder cannot work without.
        #[serde(default)]
        required_cameras: Vec<String>,
        #[serde(default)]
        on_interruption: OnInterruption,
    },
    /// Take the lease from whoever holds it, stopping every arm first.
    #[schemars(title = "TakeLease")]
    TakeLease {
        #[serde(default)]
        request_id: Option<String>,
        holder: LeaseHolder,
        label: String,
        /// Cameras the holder cannot work without.
        #[serde(default)]
        required_cameras: Vec<String>,
        #[serde(default)]
        on_interruption: OnInterruption,
    },
    /// Give the lease up; the arms keep doing what they were told.
    #[schemars(title = "ReturnLease")]
    ReturnLease,
    /// A command for one arm, answered by `command_answer`. A stop can bypass
    /// earlier requests.
    #[schemars(title = "ArmCommand")]
    Command {
        #[serde(default)]
        request_id: Option<String>,
        arm: String,
        command: StationCommand,
        /// The caller's trace context; the command's record carries it.
        #[serde(default)]
        trace_context: Option<TraceContext>,
        /// Refuse the command as `stale_revision` if the arm's revision moved on.
        #[serde(default)]
        expected_revision: Option<u64>,
    },
    /// Stop one arm only while this connection still holds the lease, answered
    /// by `command_answer`: accepted with no records once the stop is
    /// submitted, or refused as `lease_held_elsewhere` when the lease changed.
    /// A plain `stop` command is accepted from anyone.
    #[schemars(title = "StopWhileLeased")]
    StopWhileLeased {
        #[serde(default)]
        request_id: Option<String>,
        arm: String,
    },
    /// The lease holder reports whether a camera it requires gives frames,
    /// answered by `command_answer` like `stop_while_leased`. A fresh frame
    /// clears the camera's interlock.
    #[schemars(title = "ReportCamera")]
    ReportCamera {
        #[serde(default)]
        request_id: Option<String>,
        camera: String,
        available: bool,
    },
    /// Move several arms together, answered by `command_answer`.
    #[schemars(title = "MoveArms")]
    MoveArms {
        #[serde(default)]
        request_id: Option<String>,
        command: MoveArmsCommand,
        /// The caller's trace context; each arm's record carries it.
        #[serde(default)]
        trace_context: Option<TraceContext>,
        /// Per arm: refuse the move as `stale_revision` if that arm's revision
        /// moved on.
        #[serde(default)]
        expected_revisions: BTreeMap<String, u64>,
    },
    /// Where an arm would take the tool toward a pose, answered by
    /// `preview_answer`; nothing moves.
    #[schemars(title = "Preview")]
    Preview {
        #[serde(default)]
        request_id: Option<String>,
        arm: String,
        pose: PreviewPose,
    },
    /// An event from a model loop, for the viewer's live run.
    #[schemars(title = "RunEventMessage")]
    RunEvent { event: RunEvent },
    /// Trace spans from a model loop: running ones as they start, then finished.
    #[schemars(title = "RunSpansMessage")]
    RunSpans { spans: Vec<TraceSpan> },
}

/// Station event and result models shared with SDKs.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ServerMessage {
    #[schemars(title = "ActionSequenceAccepted")]
    ActionSequenceAccepted {
        request_id: Option<String>,
        action_sequence_id: String,
    },
    /// The first message: the arms this controller serves.
    #[schemars(title = "ServerHello")]
    Hello { arms: Vec<String> },
    /// One frame from one arm's link.
    #[schemars(title = "ArmFrame")]
    Frame { arm: String, frame: Box<Frame> },
    /// The answer to `acquire_lease` and `take_lease`.
    #[schemars(title = "LeaseReply")]
    Lease {
        request_id: Option<String>,
        answer: LeaseAnswer,
    },
    /// The answer to `command` or `move_arms`.
    #[schemars(title = "CommandAnswerMessage")]
    CommandAnswer {
        request_id: Option<String>,
        answer: CommandAnswer,
    },
    /// The answer to `preview`.
    #[schemars(title = "PreviewAnswerMessage")]
    PreviewAnswer {
        request_id: Option<String>,
        answer: PreviewAnswer,
    },
    /// This connection no longer holds the lease: taken, revoked, returned
    /// for an interruption, or lapsed.
    #[schemars(title = "LeaseLost")]
    LeaseLost { detail: String },
    /// The station, ten times a second.
    #[schemars(title = "StateMessage")]
    State { state: Box<StationState> },
    /// Current or most recent action sequence, sent on connection and when it changes.
    #[schemars(title = "ActionSequenceState")]
    ActionSequenceState {
        action_sequence: Option<ActionSequenceRecord>,
    },
    /// An event a model loop reported, as the server received it.
    #[schemars(title = "RunEventRecord")]
    RunEvent {
        seq: u64,
        received_unix_nano: UnixNanos,
        event: RunEvent,
    },
    /// A span as a model loop reported it; every revision arrives, unlike the
    /// retained spans in `run`.
    #[schemars(title = "RunSpanRecord")]
    RunSpan {
        seq: u64,
        received_unix_nano: UnixNanos,
        span: TraceSpan,
    },
    /// The run cursor fell out of the retained records, or is ahead of them:
    /// records after it up to `oldest_retained` are lost. The stream continues
    /// after `newest`.
    #[schemars(title = "RunResync")]
    RunResync { oldest_retained: u64, newest: u64 },
    /// The model loop's telemetry, whenever it changed.
    #[schemars(title = "RunMessage")]
    Run { run: Box<LiveRun> },
    /// A message the server could not act on; the connection stays open.
    #[schemars(title = "Refused")]
    Refused {
        request_id: Option<String>,
        detail: String,
    },
}

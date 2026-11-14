//! The state machine's state, and how it is read.

use std::collections::{HashMap, VecDeque};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use super::types::{
    GripperCommand, HeldStreamGoal, Lifecycle, Limits, Observation, Poses, PowerState, Rejection,
    ShutdownState, StreamGoal,
};
use hebbian_contracts::{ArmMode, CommandRecord, Event, TraceContext};

pub struct ArmCore {
    pub(super) limits: Limits,
    pub(super) publish: Box<dyn Fn(Event) + Send>,
    pub(super) power: PowerState,
    pub(super) lifecycle: Lifecycle,
    pub(super) records: HashMap<String, CommandRecord>,
    pub(super) record_order: VecDeque<String>,
    pub(super) last_finished: Option<String>,
    pub(super) stream: Option<HeldStreamGoal>,
    pub(super) gripper_command: Option<GripperCommand>,
    // Stop asks the jaw to hold where it is; the loop acts on it once.
    pub(super) gripper_stop_requested: bool,
    pub(super) shutdown: ShutdownState,
    /// The trace context of the request being admitted, copied onto its records.
    pub(super) trace_context: Option<TraceContext>,
    pub(super) observation: Option<Observation>,
    pub(super) segment: Option<String>,
}

/// A command or group id, unique within the process and across restarts: the
/// process's start time then its running count, in hex.
pub(crate) fn new_identifier() -> String {
    static START_SECONDS: OnceLock<u64> = OnceLock::new();
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let start = START_SECONDS.get_or_init(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs())
    });
    format!("{start:08x}{:08x}", NEXT.fetch_add(1, Ordering::Relaxed))
}

impl ArmCore {
    /// A disabled arm; `publish` is called, on the thread that owns the core, with every
    /// logged event the core raises.
    #[must_use]
    pub fn new(limits: Limits, publish: impl Fn(Event) + Send + 'static) -> Self {
        Self {
            limits,
            publish: Box::new(publish),
            power: PowerState::Disabled,
            lifecycle: Lifecycle::Idle,
            records: HashMap::new(),
            record_order: VecDeque::new(),
            last_finished: None,
            stream: None,
            gripper_command: None,
            gripper_stop_requested: false,
            shutdown: ShutdownState::NotRequested,
            trace_context: None,
            observation: None,
            segment: None,
        }
    }

    // ----- reading -----------------------------------------------------------

    #[must_use]
    pub fn mode(&self) -> ArmMode {
        self.power.mode()
    }

    #[must_use]
    pub fn fault(&self) -> Option<&str> {
        match &self.power {
            PowerState::Faulted(fault) | PowerState::Disconnected(fault) => Some(&fault.detail),
            PowerState::Disabled
            | PowerState::Holding(_)
            | PowerState::Moving(_)
            | PowerState::Reconnected => None,
        }
    }

    /// Whether a stopped arm can be handed to another controller.
    ///
    /// A stop record completes when the request is accepted, before an active
    /// motion or the jaw has necessarily reached its hold target.
    pub(crate) fn ready_for_control_handoff(&self) -> Result<bool, Rejection> {
        if self.mode().refuses_everything() {
            return Err(self.faulted_rejection());
        }
        if self.shutdown_requested() {
            return Err(Self::shutting_down());
        }
        let waiting_for_gripper_hold = self.mode() == ArmMode::Holding
            && self.limits.gripper.is_some()
            && self.gripper_stop_requested;
        Ok(!self.busy() && !waiting_for_gripper_hold)
    }

    #[must_use]
    pub(crate) fn poses(&self) -> Option<&Poses> {
        match &self.power {
            PowerState::Holding(poses) | PowerState::Moving(poses) => Some(poses),
            PowerState::Disabled
            | PowerState::Faulted(_)
            | PowerState::Disconnected(_)
            | PowerState::Reconnected => None,
        }
    }

    #[must_use]
    pub fn record(&self, command_id: &str) -> Option<&CommandRecord> {
        self.records.get(command_id)
    }

    /// The command being carried out, if any.
    #[must_use]
    pub(crate) fn active_command_id(&self) -> Option<&str> {
        match &self.lifecycle {
            Lifecycle::Starting(id)
            | Lifecycle::Executing { command_id: id, .. }
            | Lifecycle::Stopping(id) => Some(id),
            Lifecycle::Idle | Lifecycle::Queued(_) | Lifecycle::Superseding(_) => None,
        }
    }

    #[must_use]
    pub(crate) fn last_finished(&self) -> Option<&CommandRecord> {
        self.last_finished
            .as_ref()
            .and_then(|id| self.records.get(id))
    }

    #[must_use]
    pub(crate) fn segment(&self) -> Option<&str> {
        self.segment.as_deref()
    }

    /// The trace context to put on the records of the requests admitted next.
    pub fn set_trace_context(&mut self, trace_context: Option<TraceContext>) {
        self.trace_context = trace_context;
    }

    #[must_use]
    pub fn shutdown_requested(&self) -> bool {
        self.shutdown != ShutdownState::NotRequested
    }

    /// Whether a stop was asked of the active command, or shutdown has begun
    /// (except while the shutdown return to rest is running).
    #[must_use]
    pub(crate) fn stop_requested(&self) -> bool {
        self.shutdown == ShutdownState::Stopping || matches!(self.lifecycle, Lifecycle::Stopping(_))
    }

    /// Whether shutdown was asked for and has run its course: the motors are off
    /// and nothing is left to carry out.
    #[must_use]
    pub(crate) fn shutdown_finished(&self) -> bool {
        self.shutdown_requested()
            && !matches!(self.mode(), ArmMode::Holding | ArmMode::Moving)
            && !self.busy()
    }

    /// The goal the active stream follows, and when it last changed.
    #[must_use]
    pub(crate) fn stream_goal(&self, command_id: &str) -> Option<(&StreamGoal, Instant)> {
        self.stream
            .as_ref()
            .filter(|stream| stream.command_id == command_id)
            .map(|stream| (&stream.goal, stream.last_goal_at))
    }
}

use std::collections::{HashMap, VecDeque};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use super::types::{
    CommandState, GripperCommand, HeldStreamGoal, Limits, Observation, Poses, PowerState,
    Rejection, ShutdownState, StreamGoal,
};
use hebbian_contracts::{ArmMode, CommandRecord, LoggedEvent, TraceContext};

pub struct ArmCore {
    pub(crate) group_collision: Option<crate::self_collision::GroupCollisionArm>,
    pub(super) limits: Limits,
    pub(super) publish: Box<dyn Fn(LoggedEvent) + Send>,
    pub(super) power: PowerState,
    pub(super) command_state: CommandState,
    pub(super) records: HashMap<String, CommandRecord>,
    pub(super) record_order: VecDeque<String>,
    pub(super) last_finished: Option<String>,
    pub(super) stream: Option<HeldStreamGoal>,
    pub(super) gripper_command: Option<GripperCommand>,
    // INVARIANT: Stop asks the jaw to hold where it is; the loop acts on it once.
    pub(super) gripper_stop_requested: bool,
    pub(super) shutdown: ShutdownState,
    pub(super) trace_context: Option<TraceContext>,
    pub(super) observation: Option<Observation>,
    pub(super) motion_origin: Vec<f64>,
    pub(super) motion_advanced: bool,
    pub(super) segment: Option<String>,
}

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
    #[must_use]
    pub fn new(limits: Limits, publish: impl Fn(LoggedEvent) + Send + 'static) -> Self {
        Self {
            motion_origin: Vec::with_capacity(limits.joint_bounds.lower().len()),
            motion_advanced: false,
            group_collision: None,
            limits,
            publish: Box::new(publish),
            power: PowerState::Disabled,
            command_state: CommandState::Idle,
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

    // SAFETY: A stop record completes at acceptance, before motion or the jaw necessarily reaches its hold target. Handoff must also check physical readiness.
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
    pub(crate) fn gripper_command_pending(&self) -> bool {
        self.gripper_command.is_some()
    }

    pub(crate) fn stationary_joints(&self) -> Option<&[f64]> {
        if self.mode() != ArmMode::Holding || self.command_state != CommandState::Idle {
            return None;
        }
        self.observation
            .as_ref()
            .map(|observed| observed.positions.as_slice())
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

    #[must_use]
    pub(crate) fn active_command_id(&self) -> Option<&str> {
        match &self.command_state {
            CommandState::Starting(id)
            | CommandState::Executing { command_id: id, .. }
            | CommandState::Stopping(id) => Some(id),
            CommandState::Idle | CommandState::Queued(_) | CommandState::Superseding(_) => None,
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

    pub fn set_trace_context(&mut self, trace_context: Option<TraceContext>) {
        self.trace_context = trace_context;
    }

    #[must_use]
    pub fn shutdown_requested(&self) -> bool {
        self.shutdown != ShutdownState::NotRequested
    }

    #[must_use]
    pub(crate) fn stop_requested(&self) -> bool {
        self.shutdown == ShutdownState::Stopping
            || matches!(self.command_state, CommandState::Stopping(_))
    }

    #[must_use]
    pub(crate) fn shutdown_finished(&self) -> bool {
        self.shutdown_requested()
            && !matches!(self.mode(), ArmMode::Holding | ArmMode::Moving)
            && !self.busy()
    }

    #[must_use]
    pub(crate) fn stream_goal(&self, command_id: &str) -> Option<(&StreamGoal, Instant)> {
        self.stream
            .as_ref()
            .filter(|stream| stream.command_id == command_id)
            .map(|stream| (&stream.goal, stream.last_goal_at))
    }
}

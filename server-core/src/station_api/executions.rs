use std::collections::VecDeque;
use std::sync::Arc;

use hebbian_contracts::{ActionSequencePhase, ActionSequenceRecord};
use tokio::sync::watch;

#[derive(Clone)]
pub(super) struct ExecutionProgress {
    updates: Arc<watch::Sender<ActionSequenceRecord>>,
}

pub(super) struct ActiveExecution {
    progress: ExecutionProgress,
    cancellation: watch::Sender<bool>,
    owner: String,
}

#[derive(Default)]
pub(super) struct ExecutionRegistry {
    sequences: VecDeque<ActiveExecution>,
}

impl ExecutionRegistry {
    pub fn cancel_owner(&self, token: &str) -> bool {
        let mut cancelled = false;
        for active in &self.sequences {
            if active.owner == token && active.cancel() {
                cancelled = true;
            }
        }
        cancelled
    }

    pub(super) fn check_command_conflict(
        &self,
        token: Option<&str>,
        arms: &[&str],
    ) -> Result<(), &'static str> {
        if token.is_some_and(|token| self.owner_has_running_on_arms(token, arms)) {
            Err("An action sequence is running; cancel or stop it before sending another command")
        } else {
            Ok(())
        }
    }

    #[must_use]
    pub fn owner_has_running_on_arms(&self, token: &str, arms: &[&str]) -> bool {
        self.sequences
            .iter()
            .any(|active| active.owner == token && active.is_running_on(arms))
    }

    pub(super) fn install(
        &mut self,
        action_sequence: ActiveExecution,
    ) -> Result<(), ExecutionRefusal> {
        let arms = action_sequence.arms();
        if self
            .sequences
            .iter()
            .any(|active| active.is_running_on(&arms))
        {
            return Err(ExecutionRefusal::ArmBusy);
        }
        while self.sequences.len() >= 100 {
            let Some(index) = self
                .sequences
                .iter()
                .position(|active| !active.is_running())
            else {
                break;
            };
            self.sequences.remove(index);
        }
        self.sequences.push_back(action_sequence);
        Ok(())
    }

    #[must_use]
    pub fn lookup(&self, id: &str) -> Option<ActionSequenceRecord> {
        self.sequences
            .iter()
            .find_map(|active| active.record_for_id(id))
    }

    pub(super) fn progress_for(&self, id: &str) -> Option<watch::Receiver<ActionSequenceRecord>> {
        self.sequences
            .iter()
            .find(|active| active.has_id(id))
            .map(ActiveExecution::subscribe)
    }

    #[must_use]
    pub fn current(&self) -> Option<watch::Receiver<ActionSequenceRecord>> {
        self.sequences.back().map(ActiveExecution::subscribe)
    }

    pub fn cancel(&self, id: &str) -> bool {
        self.sequences
            .iter()
            .find(|active| active.has_id(id))
            .is_some_and(ActiveExecution::cancel)
    }
}

impl ActiveExecution {
    fn is_running(&self) -> bool {
        matches!(
            self.progress.updates.borrow().phase,
            ActionSequencePhase::Running { .. }
        )
    }

    fn is_running_on(&self, arms: &[impl AsRef<str>]) -> bool {
        let record = self.progress.updates.borrow();
        matches!(record.phase, ActionSequencePhase::Running { .. })
            && arms.iter().any(|arm| {
                record
                    .arms
                    .iter()
                    .any(|active_arm| active_arm == arm.as_ref())
            })
    }

    fn arms(&self) -> Vec<String> {
        self.progress.updates.borrow().arms.clone()
    }

    fn record_for_id(&self, id: &str) -> Option<ActionSequenceRecord> {
        let record = self.progress.updates.borrow();
        (record.id == id).then(|| record.clone())
    }

    fn has_id(&self, id: &str) -> bool {
        self.progress.updates.borrow().id == id
    }

    fn subscribe(&self) -> watch::Receiver<ActionSequenceRecord> {
        self.progress.subscribe()
    }

    #[must_use]
    pub fn new(record: ActionSequenceRecord, owner: String) -> (Self, watch::Receiver<bool>) {
        let (cancellation, receiver) = watch::channel(false);
        (
            Self {
                progress: ExecutionProgress::new(record),
                cancellation,
                owner,
            },
            receiver,
        )
    }

    pub(super) fn progress(&self) -> ExecutionProgress {
        self.progress.clone()
    }

    fn cancel(&self) -> bool {
        // INVARIANT: Keep completion and the decision to cancel ordered.
        let record = self.progress.updates.borrow();
        if !matches!(record.phase, ActionSequencePhase::Running { .. }) {
            return false;
        }
        self.cancellation.send_replace(true);
        true
    }
}

impl ExecutionProgress {
    #[must_use]
    fn new(record: ActionSequenceRecord) -> Self {
        let (updates, _) = watch::channel(record);
        Self {
            updates: Arc::new(updates),
        }
    }

    #[must_use]
    pub fn subscribe(&self) -> watch::Receiver<ActionSequenceRecord> {
        self.updates.subscribe()
    }

    pub fn update(&self, update: impl FnOnce(&mut ActionSequenceRecord)) {
        self.updates.send_modify(update);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ExecutionRefusal {
    PriorUnknown,
    PriorRunning,
    StepAlreadyAttempted,
    NoRemainingStep,
    PrefixChanged,
    ArmBusy,
    LeaseLost,
    UnknownArm(String),
    Interlocked(hebbian_contracts::Interlock),
}

impl std::fmt::Display for ExecutionRefusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownArm(arm) => write!(formatter, "No arm named {arm}"),
            Self::Interlocked(interlock) => interlock.fmt(formatter),
            Self::PriorUnknown => formatter.write_str("Prior execution is unknown or expired"),
            Self::PriorRunning => formatter.write_str("Finish or cancel the prior execution before continuing"),
            Self::StepAlreadyAttempted => formatter.write_str("Continuation cannot repeat an attempted step; inspect partial outcomes and submit new work explicitly"),
            Self::NoRemainingStep => formatter.write_str("Continuation must select at least one remaining step"),
            Self::PrefixChanged => formatter.write_str("Continuation must preserve the consumed source prefix"),
            Self::ArmBusy => formatter.write_str("An action sequence is already running on this arm"),
            Self::LeaseLost => formatter.write_str("The lease was returned, taken or revoked"),
        }
    }
}

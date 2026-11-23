use std::collections::BTreeMap;
use std::sync::Arc;

use crate::action_step::PreparedArmAction;
use crate::control_link::LinkHandle;
use crate::lease::{ArmGate, GenerationTicket};
use hebbian_contracts::{
    ActionSequencePhase, ActionSequenceRecord, ActionSequenceRequest, ArmMode, ArmRecord,
    CommandOutcome, CommandRecord, ExecutionFailure, RejectionReason,
};
use tokio::sync::watch;
use tokio::time::{Duration, Instant, sleep_until};

use super::programs::ProgramArtifact;
use crate::compiler::{CompiledStep, requested_joints};

struct ProgramExecution {
    continuation: Option<hebbian_contracts::ProgramContinuation>,
    artifact: Arc<ProgramArtifact>,
    expected_revisions: BTreeMap<String, u64>,
}

fn prepare_step(
    links: &BTreeMap<String, LinkHandle>,
    commands: &CompiledStep,
    source_step: usize,
) -> Result<Vec<PreparedArmAction>, ExecutionFailure> {
    let mut actions = Vec::new();
    for (arm, commands) in commands {
        let link = links
            .get(arm)
            .ok_or_else(|| ExecutionFailure::Preparation {
                step: source_step,
                arm: arm.clone(),
                reason: RejectionReason::InvalidTarget,
                detail: format!("No arm named {arm}"),
            })?;
        let action = super::commands::prepare_compiled_action(link, arm, &commands.action)
            .map_err(|(reason, detail)| ExecutionFailure::Preparation {
                step: source_step,
                arm: arm.clone(),
                reason,
                detail,
            })?;
        actions.push(action);
    }
    Ok(actions)
}

struct PreparationSource {
    artifact: Arc<ProgramArtifact>,
    execution_id: String,
    step: usize,
}

struct ExecutionAdmission {
    owner: super::admission::AdmissionOwner,
    links: BTreeMap<String, LinkHandle>,
    token: String,
    gates: Vec<ArmGate>,
    generation: Vec<GenerationTicket>,
}

// INVARIANT: Plan all commands before crossing the single atomic controller admission boundary.
async fn submit_step(
    admission: Arc<ExecutionAdmission>,
    source: PreparationSource,
    cancellation: watch::Receiver<bool>,
) -> Result<Vec<ArmRecord>, ExecutionFailure> {
    let source_step = source.step;
    let (actions, source, preparation_seconds) = tokio::task::spawn_blocking({
        let admission = Arc::clone(&admission);
        move || {
            let commands = &source.artifact.compilation.compiled.steps[source_step];
            let preparation_started = source
                .artifact
                .directory()
                .map(|_| std::time::Instant::now());
            let actions = prepare_step(&admission.links, commands, source_step)?;
            let preparation_seconds =
                preparation_started.map(|started| started.elapsed().as_secs_f64());
            if preparation_seconds.is_some() {
                let source_operations = commands
                    .iter()
                    .map(|(arm, operation)| (arm.as_str(), operation.source_operation_id.as_str()))
                    .collect();
                if let Err(error) = source.artifact.record_prepared_step(
                    &source.execution_id,
                    source.step,
                    &source_operations,
                    &actions,
                ) {
                    tracing::error!(%error, "Cannot serialize prepared step");
                }
            }
            Ok((actions, source, preparation_seconds))
        }
    })
    .await
    .map_err(|error| ExecutionFailure::AdmissionUnknown {
        step: source_step,
        detail: format!("Preparation worker failed: {error}"),
    })??;
    let admission_started = preparation_seconds.map(|seconds| (seconds, std::time::Instant::now()));
    let result = admit_prepared_step(admission, source_step, actions, cancellation).await;
    if let Some((preparation_seconds, admission_started)) = admission_started
        && let Err(error) = source.artifact.record_step_timing(
            &source.execution_id,
            source_step,
            preparation_seconds,
            admission_started.elapsed().as_secs_f64(),
        )
    {
        tracing::error!(%error, "Cannot serialize step timing");
    }
    result
}

async fn admit_prepared_step(
    admission: Arc<ExecutionAdmission>,
    source_step: usize,
    actions: Vec<PreparedArmAction>,
    cancellation: watch::Receiver<bool>,
) -> Result<Vec<ArmRecord>, ExecutionFailure> {
    let owner = admission.owner.clone();
    let reply = owner
        .decide(move |state| {
            let first = actions
                .first()
                .and_then(|action| admission.links.get(action.arm()))
                .ok_or_else(|| ExecutionFailure::Runtime {
                    step: source_step,
                    detail: "A prepared step needs an arm".into(),
                })?;
            first
                .action_step_async(
                    actions,
                    crate::control_link::SubmissionAuthority {
                        token: Some(&admission.token),
                        generation: &admission.generation,
                        clock: state.clock,
                    },
                    &admission.gates,
                    None,
                    &cancellation,
                )
                .map_err(|error| ExecutionFailure::Admission {
                    step: source_step,
                    reason: error.reason,
                    detail: error.detail,
                })
        })
        .await
        .map_err(|_| admission_lost(source_step))??;
    reply
        .await
        .map_err(|_| ExecutionFailure::AdmissionUnknown {
            step: source_step,
            detail: "Controller admission reply was lost; the step may have acted".into(),
        })?
        .map_err(|error| ExecutionFailure::Admission {
            step: source_step,
            reason: error.reason,
            detail: error.detail,
        })
}

async fn execute_program(
    admission: Arc<ExecutionAdmission>,
    sequence: ProgramExecution,
    progress: &ExecutionProgress,
    cancellation: watch::Receiver<bool>,
) {
    let ProgramExecution {
        continuation,
        artifact,
        expected_revisions,
    } = sequence;
    let links = &admission.links;
    let (notifications, updates) = watch::channel(());
    let mut watchers = tokio::task::JoinSet::new();
    for arm in expected_revisions.keys() {
        let mut changes = links[arm].changes();
        let notifications = notifications.clone();
        watchers.spawn(async move {
            while changes.changed().await.is_ok() {
                if notifications.send(()).is_err() {
                    break;
                }
            }
        });
    }
    let steps = &artifact.compilation.compiled.steps;
    let arms = &artifact.compilation.compiled.arms;
    let lease_link = &links[&arms[0]];
    let mut wait = ExecutionWait {
        admission: &admission,
        authority_changes: lease_link.authority_changes(),
        cancellation,
        updates,
    };
    let execution_id = progress.subscribe().borrow().id.clone();
    artifact.persist_execution_updates(progress.subscribe());
    let result = async {
        let step_count = steps.len();
        let first_step = continuation
            .as_ref()
            .map_or(0, |continuation| continuation.next_step);
        for (source_step, commands) in steps.iter().enumerate().skip(first_step) {
            wait.check_cancellation(source_step)?;
            let records = submit_step(
                Arc::clone(&admission),
                PreparationSource {
                    artifact: Arc::clone(&artifact),
                    execution_id: execution_id.clone(),
                    step: source_step,
                },
                wait.cancellation.clone(),
            )
            .await
            .map_err(|failure| match failure {
                ExecutionFailure::AdmissionUnknown { .. } => failure,
                _ => wait
                    .check_cancellation(source_step)
                    .err()
                    .unwrap_or(failure),
            })?;
            let records = match wait.for_step(records, source_step).await {
                Ok(records) => records,
                Err((detail, records)) => {
                    update_execution(&admission.owner, progress, move || {
                        ExecutionUpdate::StepInterrupted(records)
                    })
                    .await
                    .map_err(|_| admission_lost(source_step))??;
                    return Err(detail);
                }
            };
            let step_admission = Arc::clone(&admission);
            let cancellation = wait.cancellation.clone();
            let commands = commands.clone();
            let advance = update_execution(&admission.owner, progress, move || {
                let authority = execution_authority(&step_admission, &cancellation);
                ExecutionUpdate::StepFinished {
                    step: source_step,
                    records,
                    step_count,
                    commands,
                    authority,
                }
            })
            .await
            .map_err(|_| admission_lost(source_step))??;
            if advance == StepAdvance::End {
                break;
            }
            wait.for_holding(arms, source_step).await?;
        }
        Ok::<(), ExecutionFailure>(())
    }
    .await;
    if let Err(failure) = result {
        stop_program_motion(&admission, arms).await;
        let _ = update_execution(&admission.owner, progress, move || {
            ExecutionUpdate::Stopped(failure)
        })
        .await;
    }
}

fn admission_lost(step: usize) -> ExecutionFailure {
    ExecutionFailure::AdmissionUnknown {
        step,
        detail: "Station admission became unavailable".into(),
    }
}

enum ExecutionUpdate {
    StepInterrupted(Vec<ArmRecord>),
    StepFinished {
        step: usize,
        records: Vec<ArmRecord>,
        step_count: usize,
        commands: CompiledStep,
        authority: ExecutionAuthority,
    },
    Stopped(ExecutionFailure),
}

#[derive(Debug, PartialEq, Eq)]
enum StepAdvance {
    Continue,
    End,
}

impl ExecutionUpdate {
    fn apply(self, record: &mut ActionSequenceRecord) -> Result<StepAdvance, ExecutionFailure> {
        match self {
            Self::StepInterrupted(records) => {
                record.steps.push(records);
                Ok(StepAdvance::End)
            }
            Self::StepFinished {
                step,
                records,
                step_count,
                commands,
                authority,
            } => {
                if let Err(failure) = authority.require(step) {
                    record.steps.push(records);
                    return Err(failure);
                }
                Ok(finish_step(record, records, step_count, &commands))
            }
            Self::Stopped(failure) => {
                record.phase = ActionSequencePhase::Stopped { failure };
                Ok(StepAdvance::End)
            }
        }
    }
}

async fn update_execution(
    owner: &super::admission::AdmissionOwner,
    progress: &ExecutionProgress,
    observe: impl FnOnce() -> ExecutionUpdate + Send + 'static,
) -> Result<Result<StepAdvance, ExecutionFailure>, super::admission::AdmissionClosed> {
    let progress = progress.clone();
    owner
        .decide(move |state| {
            let event = observe();
            let mut result = Ok(StepAdvance::End);
            progress.update(|record| {
                result = event.apply(record);
                state.publish_execution(record.clone());
            });
            result
        })
        .await
}

async fn stop_program_motion(admission: &ExecutionAdmission, arms: &[String]) {
    // INVARIANT: Keep these arms reserved while their stops settle.
    let mut settling_arms = tokio::task::JoinSet::new();
    for (arm, generation) in arms.iter().zip(&admission.generation) {
        let link = admission.links[arm].clone();
        if let Some(reply) = link.stop_execution(&admission.token, generation) {
            settling_arms.spawn(async move {
                if matches!(reply.await, Ok(Ok(_))) {
                    link.wait_for_execution_stop().await;
                }
            });
        }
    }
    settling_arms.join_all().await;
}

fn check_execution_cancellation(
    cancellation: &watch::Receiver<bool>,
    source_step: usize,
) -> Result<(), ExecutionFailure> {
    if *cancellation.borrow() {
        return Err(cancelled_execution(source_step));
    }
    Ok(())
}

fn cancelled_execution(step: usize) -> ExecutionFailure {
    ExecutionFailure::Cancellation {
        step,
        detail: "Program execution canceled".into(),
    }
}

struct ExecutionWait<'a> {
    admission: &'a ExecutionAdmission,
    authority_changes: watch::Receiver<()>,
    cancellation: watch::Receiver<bool>,
    updates: watch::Receiver<()>,
}

impl ExecutionWait<'_> {
    fn check_cancellation(&self, source_step: usize) -> Result<(), ExecutionFailure> {
        check_execution_cancellation(&self.cancellation, source_step)
    }

    async fn for_change(&mut self, source_step: usize) -> Result<(), ExecutionFailure> {
        tokio::select! {
            result = self.cancellation.changed() => result,
            result = self.authority_changes.changed() => result,
            result = self.updates.changed() => result,
        }
        .map_err(|_| ExecutionFailure::Runtime {
            step: source_step,
            detail: "Execution update channel closed".into(),
        })
    }

    async fn for_holding(
        &mut self,
        arms: &[String],
        source_step: usize,
    ) -> Result<(), ExecutionFailure> {
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            let authority = execution_authority(self.admission, &self.cancellation);
            let settled = arms
                .iter()
                .all(|arm| self.admission.links[arm].current_mode() == Some(ArmMode::Holding));
            match holding_status(source_step, authority, settled, Instant::now(), deadline)? {
                WaitDecision::Ready => return Ok(()),
                WaitDecision::Pending => {}
            }
            tokio::select! {
                () = sleep_until(deadline) => {}
                result = self.for_change(source_step) => result?,
            }
        }
    }

    async fn for_step(
        &mut self,
        mut records: Vec<ArmRecord>,
        source_step: usize,
    ) -> Result<Vec<ArmRecord>, (ExecutionFailure, Vec<ArmRecord>)> {
        loop {
            for record in &mut records {
                if record.record.is_pending()
                    && let Some(updated) = self.admission.links[&record.arm]
                        .find_record(record.record.command_id.clone())
                        .await
                {
                    record.record = updated;
                }
            }
            let authority = execution_authority(self.admission, &self.cancellation);
            match step_status(source_step, authority, &records) {
                Ok(WaitDecision::Ready) => return Ok(records),
                Ok(WaitDecision::Pending) => {}
                Err(failure) => return Err((failure, records)),
            }
            if let Ok(Err(failure)) =
                tokio::time::timeout(Duration::from_secs(1), self.for_change(source_step)).await
            {
                return Err((failure, records));
            }
        }
    }
}

fn execution_authority(
    admission: &ExecutionAdmission,
    cancellation: &watch::Receiver<bool>,
) -> ExecutionAuthority {
    if *cancellation.borrow() {
        ExecutionAuthority::Cancelled
    } else if admission
        .links
        .values()
        .next()
        .is_some_and(|link| link.execution_authorized(&admission.token, &admission.gates))
    {
        ExecutionAuthority::Current
    } else {
        ExecutionAuthority::Lost
    }
}

#[derive(Clone, Copy)]
enum ExecutionAuthority {
    Current,
    Cancelled,
    Lost,
}

impl ExecutionAuthority {
    fn require(self, step: usize) -> Result<(), ExecutionFailure> {
        match self {
            Self::Current => Ok(()),
            Self::Cancelled => Err(cancelled_execution(step)),
            Self::Lost => Err(ExecutionFailure::Authority {
                step,
                detail: "The lease or an arm's interlock revision changed during execution".into(),
            }),
        }
    }
}

enum WaitDecision {
    Pending,
    Ready,
}

fn holding_status(
    step: usize,
    authority: ExecutionAuthority,
    settled: bool,
    now: Instant,
    deadline: Instant,
) -> Result<WaitDecision, ExecutionFailure> {
    authority.require(step)?;
    if settled {
        Ok(WaitDecision::Ready)
    } else if now >= deadline {
        Err(ExecutionFailure::Execution {
            step,
            detail: "Arms did not report holding after the step".into(),
        })
    } else {
        Ok(WaitDecision::Pending)
    }
}

fn step_status(
    step: usize,
    authority: ExecutionAuthority,
    records: &[ArmRecord],
) -> Result<WaitDecision, ExecutionFailure> {
    authority.require(step)?;
    if records.is_empty() {
        return Err(ExecutionFailure::AdmissionUnknown {
            step,
            detail: "Controller admitted the step without command records".into(),
        });
    }
    if records
        .iter()
        .any(|record| !record.record.is_pending() && !command_allows_next_step(&record.record))
    {
        let limited = records.iter().any(|record| record.record.was_limited());
        return Err(ExecutionFailure::Execution {
            step,
            detail: if limited {
                "A motion was limited before reaching the requested target; see the step result for details"
            } else {
                "A command in the step did not reach its requested target"
            }
            .into(),
        });
    }
    if records.iter().all(|record| !record.record.is_pending()) {
        Ok(WaitDecision::Ready)
    } else {
        Ok(WaitDecision::Pending)
    }
}

#[derive(Clone)]
pub struct ExecutionProgress {
    updates: Arc<watch::Sender<ActionSequenceRecord>>,
}

pub struct ActiveExecution {
    pub progress: ExecutionProgress,
    cancellation: watch::Sender<bool>,
    owner: String,
}

#[derive(Default)]
pub struct ExecutionRegistry {
    sequences: std::collections::VecDeque<ActiveExecution>,
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

    pub(crate) fn check_command_conflict(
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
        self.sequences.iter().any(|active| {
            active.owner == token
                && active.is_running()
                && active
                    .progress
                    .subscribe()
                    .borrow()
                    .arms
                    .iter()
                    .any(|arm| arms.contains(&arm.as_str()))
        })
    }

    pub fn install(&mut self, action_sequence: ActiveExecution) -> Result<(), &'static str> {
        let arms = action_sequence.progress.subscribe().borrow().arms.clone();
        if self.sequences.iter().any(|active| {
            active.is_running()
                && active
                    .progress
                    .subscribe()
                    .borrow()
                    .arms
                    .iter()
                    .any(|arm| arms.contains(arm))
        }) {
            return Err("An action sequence is already running on this arm");
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
        self.sequences.iter().find_map(|active| {
            let record = active.progress.updates.borrow();
            (record.id == id).then(|| record.clone())
        })
    }

    #[must_use]
    pub fn current(&self) -> Option<watch::Receiver<ActionSequenceRecord>> {
        self.sequences
            .back()
            .map(|active| active.progress.subscribe())
    }

    pub fn cancel(&self, id: &str) -> bool {
        self.sequences
            .iter()
            .find(|active| active.progress.updates.borrow().id == id)
            .is_some_and(ActiveExecution::cancel)
    }
}

impl ActiveExecution {
    fn is_running(&self) -> bool {
        matches!(
            self.progress.subscribe().borrow().phase,
            ActionSequencePhase::Running { .. }
        )
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
    pub fn new(record: ActionSequenceRecord) -> Self {
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

// CONSTRAINT: Gripper contact completes a step because gripping an object is an intended success.
#[must_use]
pub fn command_allows_next_step(record: &CommandRecord) -> bool {
    record.outcome() == Some(CommandOutcome::Completed) && !record.was_limited()
}

// INVARIANT: Advance only after every command in the active step has finished.
// Returns whether another step should be submitted.
fn finish_step(
    action_sequence: &mut ActionSequenceRecord,
    records: Vec<ArmRecord>,
    step_count: usize,
    commands: &CompiledStep,
) -> StepAdvance {
    let ActionSequencePhase::Running { step } = action_sequence.phase else {
        return StepAdvance::End;
    };
    if records.is_empty() || records.iter().any(|record| record.record.is_pending()) {
        return StepAdvance::End;
    }
    let limited_joints = records.iter().any(|record| {
        commands
            .get(&record.arm)
            .and_then(|operation| requested_joints(&operation.action))
            .is_some_and(|requested| {
                record.record.destination_joints().is_some_and(|target| {
                    target.len() != requested.len()
                        || target
                            .iter()
                            .zip(requested)
                            .any(|(actual, requested)| (actual - requested).abs() > 1e-9)
                })
            })
    });
    let success = !limited_joints
        && records
            .iter()
            .all(|record| command_allows_next_step(&record.record));
    action_sequence.steps.push(records);
    if !success {
        action_sequence.phase = ActionSequencePhase::Stopped {
            failure: ExecutionFailure::Execution {
                step,
                detail: if limited_joints {
                    "Joint target was limited before the requested position".into()
                } else {
                    format!("Step {} did not reach its requested target", step + 1)
                },
            },
        };
        return StepAdvance::End;
    }
    if step + 1 >= step_count {
        action_sequence.phase = ActionSequencePhase::Completed;
        return StepAdvance::End;
    }
    action_sequence.phase = ActionSequencePhase::Running { step: step + 1 };
    StepAdvance::Continue
}

pub(super) fn run_compiled_program(
    owner: super::admission::AdmissionOwner,
    executions: &mut ExecutionRegistry,
    links: &BTreeMap<String, LinkHandle>,
    token: String,
    artifact: Arc<ProgramArtifact>,
    expected_revisions: BTreeMap<String, u64>,
    continuation: Option<hebbian_contracts::ProgramContinuation>,
) -> Result<String, String> {
    if let Some(continuation) = &continuation {
        let prior = executions
            .lookup(&continuation.prior_execution_id)
            .ok_or("Prior execution is unknown or expired")?;
        check_continuation(&prior, &artifact.compilation.source, continuation.next_step)
            .map_err(str::to_owned)?;
    }
    let sequence = ProgramExecution {
        continuation,
        artifact,
        expected_revisions,
    };
    start_execution(owner, executions, links, token, sequence)
}

fn check_continuation(
    prior: &ActionSequenceRecord,
    revised: &ActionSequenceRequest,
    next_step: usize,
) -> Result<(), &'static str> {
    if matches!(prior.phase, ActionSequencePhase::Running { .. }) {
        return Err("Finish or cancel the prior execution before continuing");
    }
    let prior_start = prior
        .continuation
        .as_ref()
        .map_or(0, |continuation| continuation.next_step);
    let recorded = prior_start + prior.steps.len();
    let consumed = match &prior.phase {
        ActionSequencePhase::Stopped {
            failure: ExecutionFailure::AdmissionUnknown { step, .. },
        } => recorded.max(step + 1),
        ActionSequencePhase::Running { .. }
        | ActionSequencePhase::Completed
        | ActionSequencePhase::Stopped { .. } => recorded,
    };
    if next_step < consumed {
        return Err(
            "Continuation cannot repeat an attempted step; inspect partial outcomes and submit new work explicitly",
        );
    }
    if next_step >= revised.steps.len() {
        return Err("Continuation must select at least one remaining step");
    }
    if next_step > prior.requested_steps.len()
        || revised.steps[..next_step] != prior.requested_steps[..next_step]
    {
        return Err("Continuation must preserve the consumed source prefix");
    }
    Ok(())
}

fn start_execution(
    owner: super::admission::AdmissionOwner,
    executions: &mut ExecutionRegistry,
    links: &BTreeMap<String, LinkHandle>,
    token: String,
    mut sequence: ProgramExecution,
) -> Result<String, String> {
    let arms = &sequence.artifact.compilation.compiled.arms;
    for arm in arms {
        let link = links
            .get(arm)
            .ok_or_else(|| format!("No arm named {arm}"))?;
        let (interlocks, revision) = link.interlocks();
        if let Some(interlock) = interlocks.first() {
            return Err(interlock.to_string());
        }
        sequence
            .expected_revisions
            .entry(arm.clone())
            .or_insert(revision);
    }
    sequence
        .expected_revisions
        .retain(|arm, _| arms.contains(arm));
    let id = crate::arm::new_identifier();
    let record = ActionSequenceRecord {
        id: id.clone(),
        compilation_id: sequence.artifact.compilation.compilation_id.clone(),
        arms: arms.clone(),
        requested_steps: sequence.artifact.compilation.source.steps.clone(),
        phase: ActionSequencePhase::Running {
            step: sequence
                .continuation
                .as_ref()
                .map_or(0, |continuation| continuation.next_step),
        },
        continuation: sequence.continuation.clone(),
        steps: vec![],
    };
    let (active, cancellation) = ActiveExecution::new(record, token.clone());
    let progress = active.progress.clone();
    let generation = links[&arms[0]]
        .with_lease(&token, || {
            executions.install(active)?;
            Ok::<_, &'static str>(
                arms.iter()
                    .map(|arm| links[arm].fence_prior_submissions())
                    .collect(),
            )
        })
        .ok_or("The lease was returned, taken or revoked")??;
    let admission = Arc::new(ExecutionAdmission {
        owner,
        links: links.clone(),
        token,
        gates: sequence
            .expected_revisions
            .iter()
            .map(|(arm, revision)| links[arm].gate(Some(*revision)))
            .collect(),
        generation,
    });
    tokio::spawn(async move {
        execute_program(admission, sequence, &progress, cancellation).await;
    });
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action_step::PreparedGripper;
    use hebbian_contracts::{
        CommandCompletion, CommandPhase, CommandPose, KnownCommandOutcome, RecordedCommand,
    };

    #[tokio::test]
    async fn losing_the_lease_wakes_a_step_waiting_for_holding() {
        let config = crate::config::ControllerConfig::parse(include_str!(
            "../../tests/support/two_joint_config.json"
        ))
        .unwrap();
        let lease = Arc::new(crate::lease::Lease::new(crate::lease::LEASE));
        let link = crate::control_link::Link::new(&config, Arc::clone(&lease)).unwrap();
        let handle = link.handle();
        let grant = handle
            .acquire_lease(
                hebbian_contracts::LeaseRequest {
                    holder: hebbian_contracts::LeaseHolder::Agent,
                    label: "program".into(),
                    required_cameras: vec![],
                    on_interruption: hebbian_contracts::OnInterruption::WaitForApproval,
                },
                crate::lease::AcquireMode::WaitOrFail,
            )
            .unwrap();
        let arms = vec!["arm".into()];
        let admission = ExecutionAdmission {
            owner: super::super::admission::AdmissionOwner::start(
                &tokio::runtime::Handle::current(),
                Arc::new(super::super::events::EventJournal::default()),
            ),
            links: BTreeMap::from([("arm".into(), handle.clone())]),
            token: grant.lease_token,
            gates: vec![handle.gate(Some(0))],
            generation: vec![],
        };
        let (cancel_sender, cancellation) = watch::channel(false);
        let (notification_sender, updates) = watch::channel(());
        let mut wait = ExecutionWait {
            admission: &admission,
            authority_changes: handle.authority_changes(),
            cancellation,
            updates,
        };
        let holding = wait.for_holding(&arms, 0);
        tokio::pin!(holding);
        tokio::select! {
            biased;
            result = &mut holding => panic!("Waiting for holding ended early: {result:?}"),
            () = async {} => {},
        }

        assert!(lease.revoke());

        assert!(matches!(
            tokio::time::timeout(Duration::from_millis(500), holding).await,
            Ok(Err(ExecutionFailure::Authority { step: 0, .. }))
        ));
        drop((cancel_sender, notification_sender));
        link.close();
    }

    #[tokio::test]
    async fn a_prepared_step_is_refused_if_its_lease_ended_before_handoff() {
        let config = crate::config::ControllerConfig::parse(include_str!(
            "../../tests/support/two_joint_config.json"
        ))
        .unwrap();
        let lease = Arc::new(crate::lease::Lease::new(crate::lease::LEASE));
        let link = crate::control_link::Link::new(&config, Arc::clone(&lease)).unwrap();
        let handle = link.handle();
        let grant = handle
            .acquire_lease(
                hebbian_contracts::LeaseRequest {
                    holder: hebbian_contracts::LeaseHolder::Agent,
                    label: "program".into(),
                    required_cameras: vec![],
                    on_interruption: hebbian_contracts::OnInterruption::WaitForApproval,
                },
                crate::lease::AcquireMode::WaitOrFail,
            )
            .unwrap();
        let owner = super::super::admission::AdmissionOwner::start(
            &tokio::runtime::Handle::current(),
            Arc::new(super::super::events::EventJournal::default()),
        );
        let admission = Arc::new(ExecutionAdmission {
            owner: owner.clone(),
            links: BTreeMap::from([("arm".into(), handle.clone())]),
            token: grant.lease_token.clone(),
            gates: vec![handle.gate(Some(0))],
            generation: vec![handle.submission_generation()],
        });
        let prepared = vec![PreparedArmAction::gripper(
            "arm".into(),
            PreparedGripper {
                angle: hebbian_contracts::JawAngle::try_from(0.5).unwrap(),
                joint_speed: None,
            },
        )];
        owner
            .decide(move |state| {
                handle.release_lease_with(&grant.lease_token, || {
                    state.executions.cancel_owner(&grant.lease_token)
                })
            })
            .await
            .unwrap();

        let (_cancel, cancellation) = watch::channel(false);
        let result = admit_prepared_step(admission, 0, prepared, cancellation).await;
        assert!(
            matches!(
                result,
                Err(ExecutionFailure::Admission {
                    step: 0,
                    reason: RejectionReason::LeaseHeldElsewhere,
                    ..
                })
            ),
            "{result:?}"
        );
        link.close();
    }

    #[test]
    fn continuation_requires_a_finished_preserved_prefix_and_skips_attempted_steps() {
        let source = ActionSequenceRequest {
            steps: [0.2, 0.5, 0.8]
                .into_iter()
                .map(|opening| {
                    hebbian_contracts::ActionSequenceStep::single(
                        "arm",
                        hebbian_contracts::ActionSequenceArmAction::Gripper {
                            gripper_opening: opening,
                            max_joint_speed: None,
                        },
                    )
                })
                .collect(),
            expected_revisions: BTreeMap::new(),
        };
        let mut attempted = CommandRecord::new(
            "command".into(),
            "set_gripper",
            0.0,
            RecordedCommand::Gripper {
                target_angle_radians: 0.2,
                joint_speed: 1.0,
                stream_command_id: None,
            },
        );
        attempted.phase = CommandPhase::OutcomeUnknown {
            finished_at: 1.0,
            detail: Some("Canceled while moving".into()),
        };
        let mut prior = ActionSequenceRecord {
            id: "prior".into(),
            compilation_id: "compiled".into(),
            continuation: None,
            requested_steps: source.steps.clone(),
            arms: vec!["arm".into()],
            steps: vec![vec![ArmRecord {
                arm: "arm".into(),
                record: attempted,
            }]],
            phase: ActionSequencePhase::Running { step: 0 },
        };
        assert_eq!(
            check_continuation(&prior, &source, 1),
            Err("Finish or cancel the prior execution before continuing")
        );
        prior.phase = ActionSequencePhase::Stopped {
            failure: ExecutionFailure::Cancellation {
                step: 0,
                detail: "Canceled".into(),
            },
        };
        assert!(check_continuation(&prior, &source, 0).is_err());
        assert!(check_continuation(&prior, &source, 1).is_ok());
        let mut changed = source.clone();
        let hebbian_contracts::ActionSequenceArmAction::Gripper {
            gripper_opening, ..
        } = changed.steps[0].arms.get_mut("arm").unwrap()
        else {
            panic!("the fixture starts with a gripper action");
        };
        *gripper_opening = 0.9;
        assert_eq!(
            check_continuation(&prior, &changed, 1),
            Err("Continuation must preserve the consumed source prefix")
        );
        prior.continuation = Some(hebbian_contracts::ProgramContinuation {
            prior_execution_id: "earlier".into(),
            next_step: 1,
        });
        assert!(check_continuation(&prior, &source, 1).is_err());
        assert!(check_continuation(&prior, &source, 2).is_ok());
        assert!(check_continuation(&prior, &source, 3).is_err());
        prior.continuation = None;
        prior.steps.clear();
        prior.phase = ActionSequencePhase::Stopped {
            failure: ExecutionFailure::AdmissionUnknown {
                step: 0,
                detail: "Admission reply was lost".into(),
            },
        };
        assert!(check_continuation(&prior, &source, 0).is_err());
        assert!(check_continuation(&prior, &source, 1).is_ok());
    }

    #[test]
    fn registry_keeps_running_action_sequence_exclusive_and_cancels_by_id() {
        let mut registry = ExecutionRegistry::default();
        let record = || ActionSequenceRecord {
            continuation: None,
            compilation_id: "compilation".into(),
            id: "first".into(),
            arms: vec!["arm".into()],
            requested_steps: vec![],
            phase: ActionSequencePhase::Running { step: 0 },
            steps: vec![],
        };
        let (first, receiver) = ActiveExecution::new(record(), "owner".into());
        let progress = first.progress.clone();
        registry.install(first).unwrap();
        let (second, _) = ActiveExecution::new(record(), "owner".into());
        assert!(registry.install(second).is_err());
        assert!(!registry.cancel("other"));
        assert!(!*receiver.borrow());
        assert!(registry.cancel("first"));
        assert!(*receiver.borrow());
        progress.update(|record| record.phase = ActionSequencePhase::Completed);
        let mut second_record = record();
        second_record.id = "second".into();
        let (second, _) = ActiveExecution::new(second_record, "owner".into());
        assert!(registry.install(second).is_ok());
        assert!(matches!(
            registry.lookup("first").unwrap().phase,
            ActionSequencePhase::Completed
        ));
    }

    #[test]
    fn independent_arms_keep_their_own_sequences() {
        let mut registry = ExecutionRegistry::default();
        let new_sequence = |id: &str, arm: &str| {
            ActiveExecution::new(
                ActionSequenceRecord {
                    continuation: None,
                    compilation_id: "compilation".into(),
                    id: id.into(),
                    arms: vec![arm.into()],
                    requested_steps: vec![],
                    phase: ActionSequencePhase::Running { step: 0 },
                    steps: vec![],
                },
                "owner".into(),
            )
            .0
        };
        let left = new_sequence("left-sequence", "left");
        let left_progress = left.progress.clone();
        let right = new_sequence("right-sequence", "right");
        let right_progress = right.progress.clone();
        registry.install(left).unwrap();
        registry.install(right).unwrap();
        assert!(registry.lookup("left-sequence").is_some());
        assert!(registry.lookup("right-sequence").is_some());
        assert!(registry.owner_has_running_on_arms("owner", &["left"]));
        assert!(!registry.owner_has_running_on_arms("owner", &["other"]));
        left_progress.update(|record| record.phase = ActionSequencePhase::Completed);
        right_progress.update(|record| record.phase = ActionSequencePhase::Completed);
        assert!(matches!(
            registry.lookup("left-sequence").unwrap().phase,
            ActionSequencePhase::Completed
        ));
    }

    #[test]
    fn only_completed_unlimited_commands_allow_advancing() {
        let mut command = CommandRecord::new(
            "1".into(),
            "move_to_pose",
            0.0,
            RecordedCommand::PolicyReach {
                goal_pose: CommandPose {
                    position: [0.0; 3],
                    quaternion: [0.0, 0.0, 0.0, 1.0],
                },
            },
        );
        assert!(!command_allows_next_step(&command));
        command.phase = CommandPhase::Finished {
            completion: CommandCompletion {
                finished_at: 1.0,
                outcome: KnownCommandOutcome::Completed,
                detail: None,
                gripper: None,
            },
            achieved: None,
        };
        assert!(command_allows_next_step(&command));
        command.phase = CommandPhase::OutcomeUnknown {
            finished_at: 1.0,
            detail: None,
        };
        assert!(!command_allows_next_step(&command));
    }

    #[test]
    fn action_sequence_stops_after_an_interrupted_step() {
        let mut action_sequence = ActionSequenceRecord {
            continuation: None,
            compilation_id: "compilation".into(),
            id: "s".into(),
            arms: vec!["arm".into()],
            requested_steps: vec![],
            phase: ActionSequencePhase::Running { step: 0 },
            steps: vec![],
        };
        let mut command = CommandRecord::new(
            "1".into(),
            "move_to_pose",
            0.0,
            RecordedCommand::PolicyReach {
                goal_pose: CommandPose {
                    position: [0.0; 3],
                    quaternion: [0.0, 0.0, 0.0, 1.0],
                },
            },
        );
        command.phase = CommandPhase::Finished {
            completion: CommandCompletion {
                finished_at: 1.0,
                outcome: KnownCommandOutcome::Completed,
                detail: None,
                gripper: None,
            },
            achieved: None,
        };
        assert_eq!(
            finish_step(
                &mut action_sequence,
                vec![ArmRecord {
                    arm: "arm".into(),
                    record: command.clone()
                }],
                3,
                &BTreeMap::new(),
            ),
            StepAdvance::Continue
        );
        let mut after_lease_loss = action_sequence.clone();
        let late_completion = ExecutionUpdate::StepFinished {
            step: 1,
            records: vec![ArmRecord {
                arm: "arm".into(),
                record: command.clone(),
            }],
            step_count: 3,
            commands: BTreeMap::new(),
            authority: ExecutionAuthority::Lost,
        };
        assert!(matches!(
            late_completion.apply(&mut after_lease_loss),
            Err(ExecutionFailure::Authority { step: 1, .. })
        ));
        assert!(matches!(
            after_lease_loss.phase,
            ActionSequencePhase::Running { step: 1 }
        ));
        assert_eq!(after_lease_loss.steps.len(), 2);
        command.phase = CommandPhase::OutcomeUnknown {
            finished_at: 1.0,
            detail: None,
        };
        assert_eq!(
            finish_step(
                &mut action_sequence,
                vec![ArmRecord {
                    arm: "arm".into(),
                    record: command
                }],
                3,
                &BTreeMap::new(),
            ),
            StepAdvance::End
        );
        assert!(matches!(
            action_sequence.phase,
            ActionSequencePhase::Stopped { .. }
        ));
        assert_eq!(action_sequence.steps.len(), 2);
    }
}

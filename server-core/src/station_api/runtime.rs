use std::collections::BTreeMap;
use std::sync::{Arc, atomic::AtomicU64};

use crate::action_step::{PreparedArmAction, PreparedGripper, PreparedMovement};
use crate::control_link::LinkHandle;
use crate::lease::ArmGate;
use hebbian_contracts::{
    ActionSequencePhase, ActionSequenceRecord, ActionSequenceRequest, ArmMode, ArmRecord,
    CommandOutcome, CommandRecord, ExecutionFailure, RejectionReason,
};
use tokio::sync::watch;
use tokio::time::{Duration, Instant, sleep_until};

use super::programs::ProgramArtifact;
use crate::compiler::{CompiledActionCommands, CompiledStep};

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
        let mut action = PreparedArmAction {
            arm: arm.clone(),
            movement: None,
            gripper: None,
        };
        for command in commands.action.clone().into_commands() {
            match super::commands::arm_command(link, arm, command).map_err(|(reason, detail)| {
                ExecutionFailure::Preparation {
                    step: source_step,
                    arm: arm.clone(),
                    reason,
                    detail,
                }
            })? {
                hebbian_contracts::Command::Move {
                    label,
                    plan,
                    joint_speed,
                    path_limited,
                    pose_limited,
                } => {
                    action.movement = Some(PreparedMovement::Trajectory {
                        label,
                        plan,
                        joint_speed,
                        path_limited,
                        pose_limited,
                    });
                }
                hebbian_contracts::Command::ReachPose {
                    label,
                    position,
                    orientation,
                } => {
                    action.movement = Some(PreparedMovement::PolicyPose {
                        label,
                        position,
                        orientation,
                    });
                }
                hebbian_contracts::Command::SetGripper { angle, joint_speed } => {
                    action.gripper = Some(PreparedGripper { angle, joint_speed });
                }
                _ => {
                    return Err(ExecutionFailure::Runtime {
                        step: source_step,
                        detail: "Unsupported compiled operation".into(),
                    });
                }
            }
        }
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
    links: BTreeMap<String, LinkHandle>,
    token: String,
    gates: Vec<ArmGate>,
    generation: Vec<(Arc<AtomicU64>, u64)>,
}

// INVARIANT: Plan all commands before crossing the single atomic controller admission boundary.
async fn submit_step(
    admission: Arc<ExecutionAdmission>,
    source: PreparationSource,
    cancellation: watch::Receiver<bool>,
) -> Result<Vec<ArmRecord>, ExecutionFailure> {
    let source_step = source.step;
    let (reply, source, admission_timing) = tokio::task::spawn_blocking(move || {
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
                eprintln!("Cannot serialize prepared step: {error}");
            }
        }
        let first = actions
            .first()
            .and_then(|action| admission.links.get(&action.arm))
            .ok_or_else(|| ExecutionFailure::Runtime {
                step: source_step,
                detail: "A prepared step needs an arm".into(),
            })?;
        let admission_started =
            preparation_seconds.map(|seconds| (seconds, std::time::Instant::now()));
        let reply = first
            .action_step_async(
                actions,
                Some(&admission.token),
                &admission.gates,
                &admission.generation,
                &cancellation,
            )
            .map_err(|error| ExecutionFailure::Admission {
                step: source_step,
                reason: error.reason,
                detail: error.detail,
            })?;
        Ok((reply, source, admission_started))
    })
    .await
    .map_err(|error| ExecutionFailure::AdmissionUnknown {
        step: source_step,
        detail: format!("Preparation/admission worker failed: {error}"),
    })??;
    let result = reply
        .await
        .map_err(|_| ExecutionFailure::AdmissionUnknown {
            step: source_step,
            detail: "Controller admission reply was lost; the step may have acted".into(),
        })?
        .map_err(|error| ExecutionFailure::Admission {
            step: source_step,
            reason: error.reason,
            detail: error.detail,
        });
    if let Some((preparation_seconds, admission_started)) = admission_timing
        && let Err(error) = source.artifact.record_step_timing(
            &source.execution_id,
            source_step,
            preparation_seconds,
            admission_started.elapsed().as_secs_f64(),
        )
    {
        eprintln!("Cannot serialize step timing: {error}");
    }
    result
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
    let token = &admission.token;
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
        lease_link,
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
                    progress.update(|record| record.steps.push(records));
                    return Err(detail);
                }
            };
            let mut advance = Ok(false);
            progress.update(|record| match wait.check_cancellation(source_step) {
                Ok(()) => advance = Ok(finish_step(record, records, step_count, commands)),
                Err(failure) => {
                    record.steps.push(records);
                    advance = Err(failure);
                }
            });
            if !advance? {
                break;
            }
            wait.for_holding(arms, source_step).await?;
        }
        Ok::<(), ExecutionFailure>(())
    }
    .await;
    if let Err(failure) = result {
        let stop_replies: Vec<_> = arms
            .iter()
            .zip(&admission.generation)
            .filter_map(|(arm, (_, generation))| links[arm].stop_execution(token, *generation))
            .collect();
        // INVARIANT: Keep these arms reserved until the controller has consumed their stops.
        for reply in stop_replies {
            let _ = reply.await;
        }
        progress.update(|record| record.phase = ActionSequencePhase::Stopped { failure });
    }
}

struct ExecutionWait<'a> {
    admission: &'a ExecutionAdmission,
    lease_link: &'a LinkHandle,
    authority_changes: watch::Receiver<()>,
    cancellation: watch::Receiver<bool>,
    updates: watch::Receiver<()>,
}

impl ExecutionWait<'_> {
    fn check_cancellation(&self, source_step: usize) -> Result<(), ExecutionFailure> {
        if *self.cancellation.borrow() {
            return Err(ExecutionFailure::Cancellation {
                step: source_step,
                detail: "Program execution canceled".into(),
            });
        }
        Ok(())
    }

    fn check_authority(&self, source_step: usize) -> Result<(), ExecutionFailure> {
        self.check_cancellation(source_step)?;
        if !self
            .lease_link
            .execution_authorized(&self.admission.token, &self.admission.gates)
        {
            return Err(ExecutionFailure::Authority {
                step: source_step,
                detail: "The lease or an arm's interlock revision changed during execution".into(),
            });
        }
        Ok(())
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
            self.check_authority(source_step)?;
            if arms
                .iter()
                .all(|arm| self.admission.links[arm].current_mode() == Some(ArmMode::Holding))
            {
                return Ok(());
            }
            tokio::select! {
                biased;
                () = sleep_until(deadline) => {
                    self.check_authority(source_step)?;
                    return Err(ExecutionFailure::Execution {
                        step: source_step,
                        detail: "Arms did not report holding after the step".into(),
                    });
                }
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
                if record.record.completion.is_none()
                    && let Some(updated) = self.admission.links[&record.arm]
                        .find_record(record.record.command_id.clone())
                        .await
                {
                    record.record = updated;
                }
            }
            if let Err(failure) = self.check_authority(source_step) {
                return Err((failure, records));
            }
            if records.iter().any(|record| {
                record.record.completion.is_some() && !command_allows_next_step(&record.record)
            }) {
                let failure = ExecutionFailure::Execution {
                    step: source_step,
                    detail: if records.iter().any(|record| {
                        record.record.pose_limited.is_some() || record.record.path_limited.is_some()
                    }) {
                        "A motion was limited before reaching the requested target; see the step result for details".into()
                    } else {
                        "A command in the step did not reach its requested target".into()
                    },
                };
                return Err((failure, records));
            }
            if records
                .iter()
                .all(|record| record.record.completion.is_some())
            {
                return Ok(records);
            }
            if let Ok(Err(failure)) =
                tokio::time::timeout(Duration::from_secs(1), self.for_change(source_step)).await
            {
                return Err((failure, records));
            }
        }
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
    state: std::sync::Mutex<ExecutionRegistryState>,
}

#[derive(Default)]
struct ExecutionRegistryState {
    sequences: std::collections::VecDeque<ActiveExecution>,
}

impl ExecutionRegistry {
    pub fn cancel_owner(&self, token: &str) -> bool {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut cancelled = false;
        for active in &state.sequences {
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
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.sequences.iter().any(|active| {
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

    pub fn install(&self, action_sequence: ActiveExecution) -> Result<(), &'static str> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let arms = action_sequence.progress.subscribe().borrow().arms.clone();
        if state.sequences.iter().any(|active| {
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
        while state.sequences.len() >= 100 {
            let Some(index) = state
                .sequences
                .iter()
                .position(|active| !active.is_running())
            else {
                break;
            };
            state.sequences.remove(index);
        }
        state.sequences.push_back(action_sequence);
        Ok(())
    }

    #[must_use]
    pub fn lookup(&self, id: &str) -> Option<ActionSequenceRecord> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.sequences.iter().find_map(|active| {
            let record = active.progress.updates.borrow();
            (record.id == id).then(|| record.clone())
        })
    }

    #[must_use]
    pub fn current(&self) -> Option<watch::Receiver<ActionSequenceRecord>> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .sequences
            .back()
            .map(|active| active.progress.subscribe())
    }

    #[must_use]
    pub fn snapshots(&self) -> Vec<ActionSequenceRecord> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .sequences
            .iter()
            .map(|active| active.progress.subscribe().borrow().clone())
            .collect()
    }

    pub fn cancel(&self, id: &str) -> bool {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state
            .sequences
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
    record.outcome() == Some(CommandOutcome::Completed)
        && record.pose_limited.is_none()
        && record.path_limited.is_none()
}

// INVARIANT: Advance only after every command in the active step has finished.
// Returns whether another step should be submitted.
pub fn finish_step(
    action_sequence: &mut ActionSequenceRecord,
    records: Vec<ArmRecord>,
    step_count: usize,
    commands: &CompiledStep,
) -> bool {
    let ActionSequencePhase::Running { step } = action_sequence.phase else {
        return false;
    };
    if records.is_empty()
        || records
            .iter()
            .any(|record| record.record.completion.is_none())
    {
        return false;
    }
    let limited_joints = records.iter().any(|record| {
        commands
            .get(&record.arm)
            .and_then(|operation| operation.action.requested_joints())
            .is_some_and(|requested| {
                record.record.label == "move_to_joints"
                    && record.record.target_joints.as_ref().is_some_and(|target| {
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
        return false;
    }
    if step + 1 >= step_count {
        action_sequence.phase = ActionSequencePhase::Completed;
        return false;
    }
    action_sequence.phase = ActionSequencePhase::Running { step: step + 1 };
    true
}

pub(crate) fn run_compiled_program(
    executions: &ExecutionRegistry,
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
    start_execution(executions, links, token, sequence)
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

#[expect(
    clippy::expect_used,
    reason = "with_lease invokes the callback at most once while holding admission authorization."
)]
fn start_execution(
    executions: &ExecutionRegistry,
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
    let mut active = Some(active);
    let mut installed = None;
    let mut generation = Vec::new();
    let authorized = links[&arms[0]].with_lease(&token, &mut || {
        installed = Some(executions.install(active.take().expect("called once")));
        if installed.as_ref().is_some_and(Result::is_ok) {
            generation = arms
                .iter()
                .map(|arm| links[arm].fence_prior_submissions())
                .collect();
        }
    });
    if !authorized {
        return Err("The lease was returned, taken or revoked".into());
    }
    if let Some(Err(detail)) = installed {
        return Err(detail.into());
    }
    let admission = Arc::new(ExecutionAdmission {
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
    use hebbian_contracts::CommandCompletion;

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
                false,
            )
            .unwrap();
        let arms = vec!["arm".into()];
        let admission = ExecutionAdmission {
            links: BTreeMap::from([("arm".into(), handle.clone())]),
            token: grant.lease_token,
            gates: vec![handle.gate(Some(0))],
            generation: vec![],
        };
        let (cancel_sender, cancellation) = watch::channel(false);
        let (notification_sender, updates) = watch::channel(());
        let mut wait = ExecutionWait {
            admission: &admission,
            lease_link: &handle,
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

    #[test]
    fn continuation_requires_a_finished_preserved_prefix_and_skips_attempted_steps() {
        let source = ActionSequenceRequest {
            steps: [0.2, 0.5, 0.8]
                .into_iter()
                .map(|opening| {
                    hebbian_contracts::ActionSequenceStep::single(
                        "arm",
                        hebbian_contracts::ActionSequenceArmAction {
                            movement: None,
                            gripper_opening: Some(opening),
                            max_joint_speed: None,
                        },
                    )
                })
                .collect(),
            expected_revisions: BTreeMap::new(),
        };
        let mut attempted = CommandRecord::new("command".into(), "set_gripper", 0.0, None);
        attempted.completion = Some(CommandCompletion {
            finished_at: 1.0,
            outcome: CommandOutcome::Interrupted,
            detail: Some("Canceled while moving".into()),
            gripper: None,
        });
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
        changed.steps[0]
            .arms
            .get_mut("arm")
            .unwrap()
            .gripper_opening = Some(0.9);
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
        let registry = ExecutionRegistry::default();
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
        let registry = ExecutionRegistry::default();
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
        assert_eq!(registry.snapshots().len(), 2);
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
        let mut command = CommandRecord::new("1".into(), "move_to_pose", 0.0, None);
        assert!(!command_allows_next_step(&command));
        command.completion = Some(CommandCompletion {
            finished_at: 1.0,
            outcome: CommandOutcome::Completed,
            detail: None,
            gripper: None,
        });
        assert!(command_allows_next_step(&command));
        command.completion.as_mut().unwrap().outcome = CommandOutcome::Interrupted;
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
        let mut command = CommandRecord::new("1".into(), "move_to_pose", 0.0, None);
        command.completion = Some(CommandCompletion {
            finished_at: 1.0,
            outcome: CommandOutcome::Completed,
            detail: None,
            gripper: None,
        });
        assert!(finish_step(
            &mut action_sequence,
            vec![ArmRecord {
                arm: "arm".into(),
                record: command.clone()
            }],
            3,
            &BTreeMap::new(),
        ));
        command.completion.as_mut().unwrap().outcome = CommandOutcome::Interrupted;
        assert!(!finish_step(
            &mut action_sequence,
            vec![ArmRecord {
                arm: "arm".into(),
                record: command
            }],
            3,
            &BTreeMap::new(),
        ));
        assert!(matches!(
            action_sequence.phase,
            ActionSequencePhase::Stopped { .. }
        ));
        assert_eq!(action_sequence.steps.len(), 2);
    }
}

//! Decisions shared by action sequence execution and its public progress reporting.

use hebbian_contracts::{
    ActionSequenceMovement, ActionSequencePhase, ActionSequenceRecord, ActionSequenceRequest,
    ActionSequenceStep, StationCommand,
};
use hebbian_contracts::{CommandAnswer, CommandOutcome, CommandRecord};
use std::sync::Arc;
use tokio::sync::watch;

/// Submit one step through the station's normal command admission path.
/// The caller holds the lease and is responsible for stopping it on interruption.
pub async fn submit_step(
    links: &std::collections::BTreeMap<String, crate::control_link::LinkHandle>,
    arm: &str,
    commands: Vec<StationCommand>,
    token: &str,
    expected_revision: Option<u64>,
    generation: (Arc<std::sync::atomic::AtomicU64>, u64),
) -> Result<Vec<CommandRecord>, String> {
    let Some(link) = links.get(arm) else {
        return Err(format!("No arm named {arm}"));
    };
    let mut accepted = Vec::with_capacity(commands.len());
    for command in commands {
        let links = links.clone();
        let arm = arm.to_owned();
        let token_for_command = token.to_owned();
        let generation = generation.clone();
        let answer = tokio::task::spawn_blocking(move || {
            crate::station_api::commands::answer_command(
                &links,
                &arm,
                command,
                Some(&token_for_command),
                None,
                expected_revision,
                vec![generation],
            )
        })
        .await
        .map_err(|error| error.to_string())?;
        match answer {
            CommandAnswer::Accepted { records } => {
                accepted.extend(records.into_iter().map(|record| record.record));
            }
            CommandAnswer::Refused { detail, .. } => {
                if !accepted.is_empty() {
                    let _ = link.stop_if_leased(Some(token));
                }
                return Err(detail);
            }
        }
    }
    Ok(accepted)
}

/// An action sequence whose steps have been converted to actionable commands.
struct ActionSequence {
    request: ActionSequenceRequest,
    steps: Vec<Vec<StationCommand>>,
}

impl ActionSequence {
    fn new(request: ActionSequenceRequest) -> Result<Self, &'static str> {
        let steps = action_sequence_commands(&request)?;
        Ok(Self { request, steps })
    }
}

/// Execute an action sequence under an existing lease.
async fn run_action_sequence(
    links: &std::collections::BTreeMap<String, crate::control_link::LinkHandle>,
    sequence: ActionSequence,
    token: &str,
    progress: &ActionSequenceProgress,
    mut cancellation: watch::Receiver<bool>,
    generation: (Arc<std::sync::atomic::AtomicU64>, u64),
) {
    let ActionSequence { request, steps } = sequence;
    let result = async {
        let link = links
            .get(&request.arm)
            .ok_or_else(|| format!("No arm named {}", request.arm))?;
        let step_count = steps.len();
        for commands in steps {
            if *cancellation.borrow() {
                return Err("ActionSequence canceled".to_owned());
            }
            let requested_joints = commands.iter().find_map(|command| match command {
                StationCommand::MoveToJoints { joints, .. } => Some(joints.clone()),
                _ => None,
            });
            let records = submit_step(
                links,
                &request.arm,
                commands,
                token,
                request.expected_revision,
                generation.clone(),
            )
            .await?;
            let records = match wait_for_step(link, records, token, &mut cancellation).await {
                Ok(records) => records,
                Err((detail, records)) => {
                    progress.update(|action_sequence| action_sequence.steps.push(records));
                    return Err(detail.to_owned());
                }
            };
            if requested_joints.as_ref().is_some_and(|requested| {
                records
                    .iter()
                    .find(|record| record.label == "move_to_joints")
                    .and_then(|record| record.target_joints.as_ref())
                    .is_some_and(|target| {
                        target.len() != requested.len()
                            || target
                                .iter()
                                .zip(requested)
                                .any(|(actual, requested)| (actual - requested).abs() > 1e-9)
                    })
            }) {
                progress.update(|action_sequence| {
                    action_sequence.steps.push(records);
                    action_sequence.phase = ActionSequencePhase::Stopped {
                        detail: "Joint target was limited before the requested position".into(),
                    };
                });
                break;
            }
            let mut advanced = false;
            progress.update(|record| advanced = finish_step(record, records, step_count));
            if !advanced {
                break;
            }
            // A command record can arrive before the next sampled holding state.
            let mut arm_updates = link.changes();
            tokio::time::timeout(std::time::Duration::from_secs(1), async {
                loop {
                    if link
                        .view()
                        .await
                        .and_then(|view| view.sample)
                        .is_some_and(|sample| sample.mode == hebbian_contracts::ArmMode::Holding)
                    {
                        break;
                    }
                    if arm_updates.changed().await.is_err() {
                        std::future::pending::<()>().await;
                    }
                }
            })
            .await
            .map_err(|_| "Arm did not report holding after the step".to_owned())?;
        }
        Ok::<(), String>(())
    }
    .await;
    if let Err(detail) = result {
        if let Some(link) = links.get(&request.arm) {
            let _ = link.stop_if_leased(Some(token));
        }
        progress.update(|record| record.phase = ActionSequencePhase::Stopped { detail });
    }
}

/// How often a waiting step checks whether its lease is still current.
const LEASE_CHECK_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);

/// Wait for the station's retained records to finish, or for cancellation.
pub async fn wait_for_step(
    link: &crate::control_link::LinkHandle,
    mut records: Vec<CommandRecord>,
    token: &str,
    cancellation: &mut watch::Receiver<bool>,
) -> Result<Vec<CommandRecord>, (&'static str, Vec<CommandRecord>)> {
    let mut arm_updates = link.changes();
    loop {
        if *cancellation.borrow() {
            return Err(("ActionSequence canceled", records));
        }
        if !link.with_lease(token, &mut || {}) {
            return Err((
                "The lease was returned, taken or revoked while the action sequence ran",
                records,
            ));
        }
        for record in &mut records {
            if record.completion.is_none()
                && let Some(updated) = link.find_record(record.command_id.clone()).await
            {
                *record = updated;
            }
        }
        if records
            .iter()
            .any(|record| record.completion.is_some() && !command_allows_next_step(record))
            && records.iter().any(|record| record.completion.is_none())
        {
            return Err((
                "A command in the step did not reach its requested target",
                records,
            ));
        }
        if records.iter().all(|record| record.completion.is_some()) {
            return Ok(records);
        }
        tokio::select! {
            changed = cancellation.changed() => {
                if changed.is_err() {
                    return Err(("ActionSequence cancellation channel closed", records));
                }
            }
            // A record is logged, or the lease is due for its next check.
            _ = arm_updates.changed() => {}
            () = tokio::time::sleep(LEASE_CHECK_INTERVAL) => {}
        }
    }
}

/// Latest action sequence progress, shared with execution and subscribers.
#[derive(Clone)]
pub struct ActionSequenceProgress {
    updates: Arc<watch::Sender<ActionSequenceRecord>>,
}

/// One active action sequence and its cancellation signal.
pub struct ActiveActionSequence {
    pub progress: ActionSequenceProgress,
    cancellation: watch::Sender<bool>,
    owner: String,
}

/// Active and recently completed action sequences, retained across connections.
#[derive(Default)]
pub struct ActionSequenceRegistry {
    state: std::sync::Mutex<ActionSequenceRegistryState>,
}

#[derive(Default)]
struct ActionSequenceRegistryState {
    sequences: std::collections::VecDeque<ActiveActionSequence>,
}

impl ActionSequenceRegistry {
    pub fn cancel_owner(&self, token: &str) -> bool {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut cancelled = false;
        for active in &state.sequences {
            if active.owner == token && active.is_running() {
                active.cancel();
                cancelled = true;
            }
        }
        cancelled
    }

    /// Refuse a command that would overlap a running sequence owned by this lease.
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

    /// Whether an owner is running an action sequence on one of these arms.
    #[must_use]
    pub fn owner_has_running_on_arms(&self, token: &str, arms: &[&str]) -> bool {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.sequences.iter().any(|active| {
            active.owner == token
                && active.is_running()
                && arms.contains(&active.progress.subscribe().borrow().arm.as_str())
        })
    }

    /// Install an action sequence if its arm has no active sequence.
    pub fn install(&self, action_sequence: ActiveActionSequence) -> Result<(), &'static str> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let arm = action_sequence.progress.subscribe().borrow().arm.clone();
        if state
            .sequences
            .iter()
            .any(|active| active.is_running() && active.progress.subscribe().borrow().arm == arm)
        {
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
            let record = active.progress.subscribe().borrow().clone();
            (record.id == id).then_some(record)
        })
    }

    /// The most recently submitted sequence, for the existing snapshot endpoint.
    #[must_use]
    pub fn current(&self) -> Option<watch::Receiver<ActionSequenceRecord>> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .sequences
            .back()
            .map(|active| active.progress.subscribe())
    }

    /// Snapshots of retained sequences, including independent active arms.
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

    pub fn cancel(&self, id: &str) -> Option<(String, String)> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let active = state
            .sequences
            .iter()
            .find(|active| active.is_running() && active.progress.subscribe().borrow().id == id)?;
        let arm = active.progress.subscribe().borrow().arm.clone();
        active.cancel();
        Some((active.owner.clone(), arm))
    }
}

impl ActiveActionSequence {
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
                progress: ActionSequenceProgress::new(record),
                cancellation,
                owner,
            },
            receiver,
        )
    }

    /// Signal the runner; it stops the arm only if it still owns control.
    pub fn cancel(&self) {
        self.cancellation.send_replace(true);
    }
}

impl ActionSequenceProgress {
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

/// Validate every step before admitting any command.
pub fn action_sequence_commands(
    request: &ActionSequenceRequest,
) -> Result<Vec<Vec<StationCommand>>, &'static str> {
    if request.steps.is_empty() {
        return Err("An action sequence must contain at least one step");
    }
    request.steps.iter().cloned().map(step_commands).collect()
}

/// Commands in a step address independent motion and gripper resources.
pub fn step_commands(step: ActionSequenceStep) -> Result<Vec<StationCommand>, &'static str> {
    let mut commands = Vec::with_capacity(2);
    let movement_speed = step.movement.as_ref().and_then(|movement| match movement {
        ActionSequenceMovement::Joints {
            max_joint_speed, ..
        }
        | ActionSequenceMovement::Pose {
            max_joint_speed, ..
        } => *max_joint_speed,
    });
    let shared_speed = step.max_joint_speed.or(movement_speed);
    if let Some(movement) = step.movement {
        commands.push(match movement {
            ActionSequenceMovement::Joints {
                joints,
                max_joint_speed: _,
            } => StationCommand::MoveToJoints {
                joints,
                max_joint_speed: shared_speed,
            },
            ActionSequenceMovement::Pose {
                position,
                orientation,
                max_joint_speed: _,
            } => StationCommand::MoveToPose {
                position,
                orientation,
                max_joint_speed: shared_speed,
            },
        });
    }
    if let Some(opening) = step.gripper_opening {
        commands.push(StationCommand::SetGripper {
            opening,
            max_joint_speed: shared_speed,
        });
    }
    if commands.is_empty() {
        return Err("An action sequence step must contain a movement or gripper target");
    }
    Ok(commands)
}

/// Whether a finished command achieved the request well enough to advance.
/// Gripper contact is reported as completed, since gripping is an intended use.
#[must_use]
pub fn command_allows_next_step(record: &CommandRecord) -> bool {
    record.outcome() == Some(CommandOutcome::Completed)
        && record.pose_limited.is_none()
        && record.path_limited.is_none()
}

/// Advance only after every command in the active step has finished.
/// Returns whether another step should be submitted.
pub fn finish_step(
    action_sequence: &mut ActionSequenceRecord,
    records: Vec<CommandRecord>,
    step_count: usize,
) -> bool {
    let ActionSequencePhase::Running { step } = action_sequence.phase else {
        return false;
    };
    if records.is_empty() || records.iter().any(|record| record.completion.is_none()) {
        return false;
    }
    let success = records.iter().all(command_allows_next_step);
    action_sequence.steps.push(records);
    if !success {
        action_sequence.phase = ActionSequencePhase::Stopped {
            detail: format!("Step {} did not reach its requested target", step + 1),
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

pub(crate) fn start_action_sequence(
    served: &Arc<crate::station_api::http::Served>,
    links: &std::collections::BTreeMap<String, crate::control_link::LinkHandle>,
    lease_token: Option<String>,
    request: ActionSequenceRequest,
) -> Result<String, String> {
    let mut sequence = ActionSequence::new(request).map_err(str::to_owned)?;
    let request = &mut sequence.request;
    let token = lease_token.ok_or_else(|| {
        "This connection does not hold the lease. Acquire it before starting an action sequence"
            .to_owned()
    })?;
    let link = links
        .get(&request.arm)
        .ok_or_else(|| "Unknown arm".to_owned())?;
    let (interlocks, revision) = link.interlocks();
    if let Some(interlock) = interlocks.first() {
        return Err(interlock.to_string());
    }
    request.expected_revision.get_or_insert(revision);
    let action_sequence_id = crate::arm::new_identifier();
    let record = ActionSequenceRecord {
        id: action_sequence_id.clone(),
        arm: request.arm.clone(),
        requested_steps: request.steps.clone(),
        phase: ActionSequencePhase::Running { step: 0 },
        steps: vec![],
    };
    let (active, cancellation) =
        crate::station_api::action_sequences::ActiveActionSequence::new(record, token.clone());
    let progress = active.progress.clone();
    let mut active = Some(active);
    let mut installed = None;
    let mut generation = None;
    let authorized = link.with_lease(&token, &mut || {
        installed = Some(
            served
                .action_sequences
                .install(active.take().expect("called once")),
        );
        if installed.as_ref().is_some_and(Result::is_ok) {
            generation = Some(link.fence_prior_submissions());
        }
    });
    if !authorized {
        return Err(
            "The lease was returned, taken or revoked while the action sequence ran".into(),
        );
    }
    if let Some(Err(detail)) = installed {
        return Err(detail.into());
    }
    let links = links.clone();
    tokio::spawn(async move {
        crate::station_api::action_sequences::run_action_sequence(
            &links,
            sequence,
            &token,
            &progress,
            cancellation,
            generation.expect("authorized"),
        )
        .await;
    });
    Ok(action_sequence_id)
}

/// Cancel an active sequence and stop its arm while its owner still holds the lease.
pub(crate) fn cancel_action_sequence(
    registry: &ActionSequenceRegistry,
    links: &std::collections::BTreeMap<String, crate::control_link::LinkHandle>,
    action_sequence_id: &str,
) -> bool {
    let Some((owner, arm)) = registry.cancel(action_sequence_id) else {
        return false;
    };
    if let Some(link) = links.get(&arm) {
        let _ = link.stop_if_leased(Some(&owner));
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use hebbian_contracts::CommandCompletion;

    #[test]
    fn progress_subscribers_receive_the_current_action_sequence() {
        let progress = ActionSequenceProgress::new(ActionSequenceRecord {
            id: "s".into(),
            arm: "arm".into(),
            requested_steps: vec![],
            phase: ActionSequencePhase::Running { step: 0 },
            steps: vec![],
        });
        progress.update(|record| record.phase = ActionSequencePhase::Completed);
        assert!(matches!(
            progress.subscribe().borrow().phase,
            ActionSequencePhase::Completed
        ));
    }

    #[test]
    fn registry_keeps_running_action_sequence_exclusive_and_cancels_by_id() {
        let registry = ActionSequenceRegistry::default();
        let record = || ActionSequenceRecord {
            id: "first".into(),
            arm: "arm".into(),
            requested_steps: vec![],
            phase: ActionSequencePhase::Running { step: 0 },
            steps: vec![],
        };
        let (first, receiver) = ActiveActionSequence::new(record(), "owner".into());
        let progress = first.progress.clone();
        registry.install(first).unwrap();
        let (second, _) = ActiveActionSequence::new(record(), "owner".into());
        assert!(registry.install(second).is_err());
        assert!(registry.cancel("other").is_none());
        assert!(!*receiver.borrow());
        assert!(registry.cancel("first").is_some());
        assert!(*receiver.borrow());
        progress.update(|record| record.phase = ActionSequencePhase::Completed);
        let mut second_record = record();
        second_record.id = "second".into();
        let (second, _) = ActiveActionSequence::new(second_record, "owner".into());
        assert!(registry.install(second).is_ok());
        assert!(matches!(
            registry.lookup("first").unwrap().phase,
            ActionSequencePhase::Completed
        ));
    }

    #[test]
    fn independent_arms_keep_their_own_sequences() {
        let registry = ActionSequenceRegistry::default();
        let new_sequence = |id: &str, arm: &str| {
            ActiveActionSequence::new(
                ActionSequenceRecord {
                    id: id.into(),
                    arm: arm.into(),
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
    fn step_has_one_command_per_requested_resource() {
        assert!(
            step_commands(ActionSequenceStep {
                movement: None,
                gripper_opening: None,
                max_joint_speed: None,
            })
            .is_err()
        );
        let commands = step_commands(ActionSequenceStep {
            movement: Some(ActionSequenceMovement::Joints {
                joints: vec![0.1],
                max_joint_speed: None,
            }),
            gripper_opening: Some(1.0),
            max_joint_speed: None,
        })
        .unwrap();
        assert!(matches!(
            commands.as_slice(),
            [
                StationCommand::MoveToJoints { .. },
                StationCommand::SetGripper {
                    opening: 1.0,
                    max_joint_speed: None
                }
            ]
        ));
    }

    #[test]
    fn both_resources_use_the_same_effective_step_speed() {
        let step = |gripper_speed| ActionSequenceStep {
            movement: Some(ActionSequenceMovement::Joints {
                joints: vec![0.1],
                max_joint_speed: Some(0.7),
            }),
            gripper_opening: Some(0.5),
            max_joint_speed: gripper_speed,
        };
        for (requested, expected) in [(None, 0.7), (Some(0.3), 0.3)] {
            let commands = step_commands(step(requested)).unwrap();
            assert!(
                matches!(commands[0], StationCommand::MoveToJoints { max_joint_speed: Some(speed), .. } if (speed - expected).abs() < f64::EPSILON)
            );
            assert!(
                matches!(commands[1], StationCommand::SetGripper { max_joint_speed: Some(speed), .. } if (speed - expected).abs() < f64::EPSILON)
            );
        }
    }

    #[test]
    fn only_completed_unlimited_commands_allow_advancing() {
        let mut command = CommandRecord::new("1".into(), "move_to_pose", 0.0, None);
        assert!(!command_allows_next_step(&command));
        command.completion = Some(CommandCompletion {
            finished_at: 1.0,
            outcome: CommandOutcome::Completed,
            detail: None,
        });
        assert!(command_allows_next_step(&command));
        command.completion.as_mut().unwrap().outcome = CommandOutcome::Interrupted;
        assert!(!command_allows_next_step(&command));
    }

    #[test]
    fn action_sequence_stops_after_an_interrupted_step() {
        let mut action_sequence = ActionSequenceRecord {
            id: "s".into(),
            arm: "arm".into(),
            requested_steps: vec![],
            phase: ActionSequencePhase::Running { step: 0 },
            steps: vec![],
        };
        let mut command = CommandRecord::new("1".into(), "move_to_pose", 0.0, None);
        command.completion = Some(CommandCompletion {
            finished_at: 1.0,
            outcome: CommandOutcome::Completed,
            detail: None,
        });
        assert!(finish_step(&mut action_sequence, vec![command.clone()], 3));
        command.completion.as_mut().unwrap().outcome = CommandOutcome::Interrupted;
        assert!(!finish_step(&mut action_sequence, vec![command], 3));
        assert!(matches!(
            action_sequence.phase,
            ActionSequencePhase::Stopped { .. }
        ));
        assert_eq!(action_sequence.steps.len(), 2);
    }
}

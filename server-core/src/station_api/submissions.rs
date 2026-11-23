use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use hebbian_contracts::{
    ArmRecord, CommandAnswer, CommandSubmissionRequest, GroupCommandSubmissionRequest,
    OperationStatus, StationCommand,
};
use tokio::sync::watch;
use uuid::Uuid;

const MAX_RETAINED_SUBMISSIONS: usize = 10_000;

type RequestOutcome<Reply, Failure> = Result<Reply, Failure>;

// INVARIANT: Every started submission publishes an outcome, even if its worker exits early.
pub(super) struct SubmissionCompletion<Reply, Failure> {
    outcome: Option<watch::Sender<Option<RequestOutcome<Reply, Failure>>>>,
    failure: Option<Failure>,
}

impl<Reply, Failure> SubmissionCompletion<Reply, Failure> {
    pub(super) fn start(
        outcome: watch::Sender<Option<RequestOutcome<Reply, Failure>>>,
        failure: Failure,
        start: impl FnOnce(Self),
    ) -> watch::Receiver<Option<RequestOutcome<Reply, Failure>>> {
        let response = outcome.subscribe();
        start(Self {
            outcome: Some(outcome),
            failure: Some(failure),
        });
        response
    }

    pub(super) fn publish(mut self, result: RequestOutcome<Reply, Failure>) {
        if let Some(outcome) = self.outcome.take() {
            outcome.send_replace(Some(result));
        }
    }
}

impl<Reply, Failure> Drop for SubmissionCompletion<Reply, Failure> {
    fn drop(&mut self) {
        if let (Some(outcome), Some(failure)) = (self.outcome.take(), self.failure.take()) {
            outcome.send_replace(Some(Err(failure)));
        }
    }
}

pub(super) async fn wait_for_outcome<Reply: Clone, Failure: Clone>(
    mut response: watch::Receiver<Option<RequestOutcome<Reply, Failure>>>,
) -> Result<RequestOutcome<Reply, Failure>, watch::error::RecvError> {
    loop {
        if let Some(outcome) = response.borrow_and_update().clone() {
            return Ok(outcome);
        }
        response.changed().await?;
    }
}

struct TrackedRequest<Request, Reply, Failure = String> {
    request: Request,
    outcome: watch::Sender<Option<RequestOutcome<Reply, Failure>>>,
}

// INVARIANT: Completed outcomes may be discarded, but their identities remain fenced.
pub struct RequestRegistry<Request, Reply, Failure = String> {
    entries: Mutex<SubmissionHistory<TrackedRequest<Request, Reply, Failure>>>,
}

impl<Request, Reply, Failure> Default for RequestRegistry<Request, Reply, Failure> {
    fn default() -> Self {
        Self {
            entries: Mutex::default(),
        }
    }
}

pub type LeaseSubmissions =
    RequestRegistry<hebbian_contracts::LeaseAcquireRequest, hebbian_contracts::LeaseGrant>;

#[derive(Clone, PartialEq)]
pub(crate) enum ProgramSubmissionRequest {
    Source(hebbian_contracts::ActionSequenceStartRequest),
    Compiled(hebbian_contracts::RunCompiledProgramRequest),
}

pub(crate) type ProgramSubmissions =
    RequestRegistry<ProgramSubmissionRequest, String, super::program_http::ProgramSubmissionError>;

struct SubmissionHistory<T, Id = Uuid> {
    retained: HashMap<Id, T>,
    seen: HashSet<Id>,
}

impl<T, Id> Default for SubmissionHistory<T, Id> {
    fn default() -> Self {
        Self {
            retained: HashMap::new(),
            seen: HashSet::new(),
        }
    }
}

impl<T, Id: Copy + Eq + std::hash::Hash> SubmissionHistory<T, Id> {
    fn make_room(&mut self, completed: impl Fn(&T) -> bool) -> bool {
        if self.retained.len() < MAX_RETAINED_SUBMISSIONS {
            return true;
        }
        let completed_id = self
            .retained
            .iter()
            .find_map(|(id, submission)| completed(submission).then_some(*id));
        if let Some(id) = completed_id {
            self.retained.remove(&id);
            true
        } else {
            false
        }
    }

    fn insert(&mut self, id: Id, submission: T) {
        self.seen.insert(id);
        self.retained.insert(id, submission);
    }
}

pub enum RequestReservation<Reply, Failure = String> {
    New(watch::Sender<Option<RequestOutcome<Reply, Failure>>>),
    Existing(watch::Receiver<Option<RequestOutcome<Reply, Failure>>>),
    Conflict,
    Expired,
    Full,
}

pub type LeaseReservation = RequestReservation<hebbian_contracts::LeaseGrant>;

impl<Request: PartialEq, Reply, Failure> RequestRegistry<Request, Reply, Failure> {
    pub fn reserve(&self, id: Uuid, request: Request) -> RequestReservation<Reply, Failure> {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(existing) = entries.retained.get(&id) {
            return if existing.request == request {
                RequestReservation::Existing(existing.outcome.subscribe())
            } else {
                RequestReservation::Conflict
            };
        }
        if entries.seen.contains(&id) {
            return RequestReservation::Expired;
        }
        if !entries.make_room(|submission| submission.outcome.borrow().is_some()) {
            return RequestReservation::Full;
        }
        let (outcome, _) = watch::channel(None);
        entries.insert(
            id,
            TrackedRequest {
                request,
                outcome: outcome.clone(),
            },
        );
        RequestReservation::New(outcome)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum SubmissionRequest {
    Single(CommandSubmissionRequest),
    Group(GroupCommandSubmissionRequest),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct OperationId(Uuid);

impl OperationId {
    pub(super) fn parse(value: &str) -> Option<Self> {
        let id = Uuid::parse_str(value).ok()?;
        (id.get_version_num() == 4 && id.get_variant() == uuid::Variant::RFC4122)
            .then_some(Self(id))
    }
}

pub(super) struct ControllerStreamCommand(String);

impl ControllerStreamCommand {
    pub(super) fn into_controller_id(self) -> String {
        self.0
    }
}

pub(super) type Admission = Result<CommandAnswer, CommandOutcomeUnknown>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum CommandOutcomeUnknown {
    AdmissionWorker,
    ControllerReplyLost,
    UnexpectedArmRecords,
    MissingArm(String),
    MissingCompletion(String),
    CompletionWorker(String),
}

impl std::fmt::Display for CommandOutcomeUnknown {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AdmissionWorker => {
                formatter.write_str("Command worker failed; outcome is unknown")
            }
            Self::ControllerReplyLost => {
                formatter.write_str("Control loop reply lost; command outcome is unknown")
            }
            Self::UnexpectedArmRecords => {
                formatter.write_str("Control loop returned unexpected arm records")
            }
            Self::MissingArm(arm) => write!(formatter, "Arm {arm} disappeared"),
            Self::MissingCompletion(arm) => {
                write!(formatter, "Completion for arm {arm} is unavailable")
            }
            Self::CompletionWorker(detail) => formatter.write_str(detail),
        }
    }
}

enum TerminalMotion {
    Finished(Vec<ArmRecord>),
    Refused {
        reason: hebbian_contracts::RejectionReason,
        detail: String,
    },
    CancelledBeforeAdmission,
    OutcomeUnknown(CommandOutcomeUnknown),
}

impl TerminalMotion {
    fn status(&self) -> OperationStatus {
        match self {
            Self::Finished(records) => OperationStatus::Finished {
                records: records.clone(),
            },
            Self::Refused { reason, detail } => OperationStatus::Refused {
                reason: *reason,
                detail: detail.clone(),
            },
            Self::CancelledBeforeAdmission => OperationStatus::CancelledBeforeAdmission,
            Self::OutcomeUnknown(failure) => OperationStatus::OutcomeUnknown {
                detail: failure.to_string(),
            },
        }
    }
}

enum OperationState {
    Pending(watch::Sender<bool>),
    CancellingAdmission,
    Active(Vec<ArmRecord>),
    Streaming {
        record: Box<ArmRecord>,
        newest_goal: Option<u32>,
    },
    Cancelling(Vec<ArmRecord>),
    Terminal(TerminalMotion),
}

enum CancellationEffect<'a> {
    FenceAdmission(&'a watch::Sender<bool>),
    AwaitAdmission,
    StopActive(Vec<ArmRecord>),
    RepeatStop(&'a [ArmRecord]),
    NoMotion,
}

impl OperationState {
    fn cancellation_effect(&self) -> CancellationEffect<'_> {
        match self {
            Self::Pending(fence) => CancellationEffect::FenceAdmission(fence),
            Self::CancellingAdmission => CancellationEffect::AwaitAdmission,
            Self::Active(records) => CancellationEffect::StopActive(records.clone()),
            Self::Streaming { record, .. } => {
                CancellationEffect::StopActive(vec![record.as_ref().clone()])
            }
            Self::Cancelling(records) => CancellationEffect::RepeatStop(records),
            Self::Terminal(_) => CancellationEffect::NoMotion,
        }
    }
}

struct Operation {
    request: SubmissionRequest,
    admission: watch::Sender<Option<Admission>>,
    state: OperationState,
}

impl Operation {
    fn cancel(&mut self) -> CancellationResult {
        let target = match self.state.cancellation_effect() {
            CancellationEffect::FenceAdmission(fence) => {
                // INVARIANT: Enqueue holds the watch read lock. Cancellation either
                // closes the fence first or receives the admitted controller IDs.
                fence.send_replace(true);
                self.state = OperationState::CancellingAdmission;
                CancellationTarget::AwaitAdmission(self.admission.subscribe())
            }
            CancellationEffect::AwaitAdmission => {
                CancellationTarget::AwaitAdmission(self.admission.subscribe())
            }
            CancellationEffect::StopActive(records) => {
                self.state = OperationState::Cancelling(records.clone());
                CancellationTarget::Commands(records)
            }
            CancellationEffect::RepeatStop(records) => {
                CancellationTarget::Commands(records.to_vec())
            }
            CancellationEffect::NoMotion => return CancellationResult::AlreadyStopped,
        };
        CancellationResult::Requested(target)
    }
}

enum Submission {
    Operation(Box<Operation>),
    CancelledBeforeArrival { lease_token: String },
}

impl Submission {
    fn is_evictable(&self) -> bool {
        match self {
            Self::Operation(operation) => matches!(operation.state, OperationState::Terminal(_)),
            Self::CancelledBeforeArrival { .. } => true,
        }
    }
}

#[derive(Clone, Default)]
pub struct Operations {
    entries: Arc<Mutex<SubmissionHistory<Submission, OperationId>>>,
}

// INVARIANT: Only the admission worker can commit its result, including on early exit.
pub(super) struct PendingAdmission {
    operations: Operations,
    id: Option<OperationId>,
}

impl PendingAdmission {
    pub(super) fn complete(mut self, answer: Admission) -> Vec<ArmRecord> {
        self.id
            .take()
            .map_or_else(Vec::new, |id| self.operations.record_admission(id, answer))
    }
}

impl Drop for PendingAdmission {
    fn drop(&mut self) {
        if let Some(id) = self.id.take() {
            self.operations
                .record_admission(id, Err(CommandOutcomeUnknown::AdmissionWorker));
        }
    }
}

pub(super) enum SubmissionReservation {
    New {
        pending: PendingAdmission,
        response: watch::Receiver<Option<Admission>>,
        cancellation: watch::Receiver<bool>,
    },
    Existing(watch::Receiver<Option<Admission>>),
    Cancelled,
    Conflict,
    Expired,
    Full,
}

pub(super) enum CancellationResult {
    AlreadyStopped,
    Requested(CancellationTarget),
    Expired,
    Conflict,
}

pub(super) enum CancellationTarget {
    AwaitAdmission(watch::Receiver<Option<Admission>>),
    Commands(Vec<ArmRecord>),
}

impl SubmissionRequest {
    fn lease_token(&self) -> Option<&str> {
        match self {
            Self::Single(request) => request.lease_token.as_deref(),
            Self::Group(request) => request.lease_token.as_deref(),
        }
    }

    fn starts_stream(&self) -> bool {
        matches!(self, Self::Single(request) if matches!(request.command, StationCommand::StreamJoints { .. }))
    }
}

impl Operations {
    fn record_admission(&self, id: OperationId, answer: Admission) -> Vec<ArmRecord> {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(Submission::Operation(operation)) = entries.retained.get_mut(&id) else {
            return Vec::new();
        };
        let mut unfinished = Vec::new();
        operation.state = match &answer {
            Ok(CommandAnswer::Accepted { records })
                if records.iter().any(|entry| entry.record.is_pending()) =>
            {
                unfinished.clone_from(records);
                if matches!(operation.state, OperationState::CancellingAdmission) {
                    OperationState::Cancelling(records.clone())
                } else if operation.request.starts_stream()
                    && let [record] = records.as_slice()
                {
                    OperationState::Streaming {
                        record: Box::new(record.clone()),
                        newest_goal: None,
                    }
                } else {
                    OperationState::Active(records.clone())
                }
            }
            Ok(CommandAnswer::Accepted { records }) => {
                OperationState::Terminal(TerminalMotion::Finished(records.clone()))
            }
            Ok(CommandAnswer::Refused { .. })
                if matches!(operation.state, OperationState::CancellingAdmission) =>
            {
                OperationState::Terminal(TerminalMotion::CancelledBeforeAdmission)
            }
            Ok(CommandAnswer::Refused { reason, detail }) => {
                OperationState::Terminal(TerminalMotion::Refused {
                    reason: *reason,
                    detail: detail.clone(),
                })
            }
            Err(detail) => OperationState::Terminal(TerminalMotion::OutcomeUnknown(detail.clone())),
        };
        operation.admission.send_replace(Some(answer));
        unfinished
    }

    pub(super) fn motion_finished(
        &self,
        id: OperationId,
        result: Result<Vec<ArmRecord>, CommandOutcomeUnknown>,
    ) {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(Submission::Operation(operation)) = entries.retained.get_mut(&id)
            && matches!(
                operation.state,
                OperationState::Active(_)
                    | OperationState::Streaming { .. }
                    | OperationState::Cancelling(_)
            )
        {
            operation.state = OperationState::Terminal(match result {
                Ok(records) => TerminalMotion::Finished(records),
                Err(detail) => TerminalMotion::OutcomeUnknown(detail),
            });
        }
    }

    pub(super) fn status(&self, id: OperationId) -> Option<OperationStatus> {
        let entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Some(match entries.retained.get(&id)? {
            Submission::CancelledBeforeArrival { .. } => OperationStatus::CancelledBeforeAdmission,
            Submission::Operation(operation) => match &operation.state {
                OperationState::Pending(_) => OperationStatus::Pending,
                OperationState::CancellingAdmission => OperationStatus::Cancelling {
                    records: Vec::new(),
                },
                OperationState::Active(records) => OperationStatus::Active {
                    records: records.clone(),
                },
                OperationState::Streaming { record, .. } => OperationStatus::Active {
                    records: vec![record.as_ref().clone()],
                },
                OperationState::Cancelling(records) => OperationStatus::Cancelling {
                    records: records.clone(),
                },
                OperationState::Terminal(status) => status.status(),
            },
        })
    }

    // INVARIANT: The lock orders every update enqueue against both cancellation and other
    // updates. The controller separately checks that the stream is still live.
    pub(super) fn update_stream<Reply>(
        &self,
        id: OperationId,
        token: &str,
        sequence: u32,
        enqueue: impl FnOnce(
            &str,
            ControllerStreamCommand,
        ) -> Result<Reply, (hebbian_contracts::RejectionReason, String)>,
    ) -> Result<Reply, (hebbian_contracts::RejectionReason, String)> {
        use hebbian_contracts::RejectionReason;
        let ended = || {
            (
                RejectionReason::StreamEnded,
                "Stream operation has ended".to_owned(),
            )
        };
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(Submission::Operation(operation)) = entries.retained.get_mut(&id) else {
            return Err(ended());
        };
        if operation.request.lease_token() != Some(token) {
            return Err((
                RejectionReason::LeaseHeldElsewhere,
                "Stream belongs to another lease".to_owned(),
            ));
        }
        let OperationState::Streaming {
            record,
            newest_goal,
        } = &mut operation.state
        else {
            return Err(ended());
        };
        if newest_goal.is_some_and(|latest| sequence <= latest) {
            return Err((
                RejectionReason::InvalidTarget,
                "Stream goal sequence must increase".to_owned(),
            ));
        }
        let answer = enqueue(
            &record.arm,
            ControllerStreamCommand(record.record.command_id.clone()),
        )?;
        *newest_goal = Some(sequence);
        Ok(answer)
    }

    pub(super) fn reserve(
        &self,
        id: OperationId,
        request: SubmissionRequest,
    ) -> SubmissionReservation {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(existing) = entries.retained.get(&id) {
            return match existing {
                Submission::Operation(operation) if operation.request == request => {
                    SubmissionReservation::Existing(operation.admission.subscribe())
                }
                Submission::CancelledBeforeArrival { lease_token }
                    if request.lease_token() == Some(lease_token) =>
                {
                    SubmissionReservation::Cancelled
                }
                Submission::Operation(_) | Submission::CancelledBeforeArrival { .. } => {
                    SubmissionReservation::Conflict
                }
            };
        }
        if entries.seen.contains(&id) {
            return SubmissionReservation::Expired;
        }
        if !entries.make_room(Submission::is_evictable) {
            return SubmissionReservation::Full;
        }
        let (admission, response) = watch::channel(None);
        let (cancelled, cancellation) = watch::channel(false);
        entries.insert(
            id,
            Submission::Operation(Box::new(Operation {
                request,
                admission,
                state: OperationState::Pending(cancelled),
            })),
        );
        SubmissionReservation::New {
            pending: PendingAdmission {
                operations: self.clone(),
                id: Some(id),
            },
            response,
            cancellation,
        }
    }

    pub(super) fn cancel(&self, id: OperationId, token: &str) -> CancellationResult {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match entries.retained.get_mut(&id) {
            Some(Submission::Operation(operation))
                if operation.request.lease_token() == Some(token) =>
            {
                return operation.cancel();
            }
            Some(Submission::CancelledBeforeArrival { lease_token }) if lease_token == token => {
                return CancellationResult::AlreadyStopped;
            }
            Some(_) => return CancellationResult::Conflict,
            None => {}
        }
        if entries.seen.contains(&id) {
            return CancellationResult::Expired;
        }
        if entries.make_room(Submission::is_evictable) {
            entries.insert(
                id,
                Submission::CancelledBeforeArrival {
                    lease_token: token.to_owned(),
                },
            );
        } else {
            entries.seen.insert(id);
        }
        CancellationResult::AlreadyStopped
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CancellationResult, CancellationTarget, LeaseReservation, LeaseSubmissions, Operations,
        SubmissionRequest, SubmissionReservation,
    };
    use hebbian_contracts::{
        CommandAnswer, CommandSubmissionRequest, RejectionReason, StationCommand,
    };
    use uuid::Uuid;

    #[tokio::test]
    async fn a_failed_worker_resolves_the_original_request_and_its_retry() {
        use super::{RequestRegistry, RequestReservation, SubmissionCompletion, wait_for_outcome};

        let submissions = RequestRegistry::<_, String>::default();
        let request_id = uuid::Uuid::new_v4();
        let RequestReservation::New(outcome) = submissions.reserve(request_id, "program") else {
            panic!("the first submission must reserve the request");
        };
        let response = SubmissionCompletion::start(
            outcome,
            "Program outcome is unknown".to_owned(),
            |completion| {
                let worker = tokio::spawn(async move {
                    std::future::pending::<()>().await;
                    completion.publish(Ok("execution".to_owned()));
                });
                worker.abort();
            },
        );
        let RequestReservation::Existing(retry) = submissions.reserve(request_id, "program") else {
            panic!("the retry must follow the same request");
        };
        for response in [response, retry] {
            let outcome = tokio::time::timeout(
                std::time::Duration::from_secs(1),
                wait_for_outcome(response),
            )
            .await
            .unwrap()
            .unwrap();
            assert_eq!(outcome, Err("Program outcome is unknown".to_owned()));
        }
    }

    #[test]
    fn lease_retries_share_the_original_acquisition() {
        let submissions = LeaseSubmissions::default();
        let id = Uuid::new_v4();
        let request = hebbian_contracts::LeaseAcquireRequest {
            request_id: id.to_string(),
            station_instance_id: Uuid::new_v4().to_string(),
            request: hebbian_contracts::LeaseRequest {
                holder: hebbian_contracts::LeaseHolder::Agent,
                label: "first".into(),
                required_cameras: vec![],
                on_interruption: hebbian_contracts::OnInterruption::WaitForApproval,
            },
            take: true,
        };
        let LeaseReservation::New(sender) = submissions.reserve(id, request.clone()) else {
            panic!("first request must be new");
        };
        let LeaseReservation::Existing(receiver) = submissions.reserve(id, request.clone()) else {
            panic!("retry must follow the first request");
        };
        sender.send_replace(Some(Err("refused".into())));
        assert_eq!(
            receiver.borrow().as_ref().unwrap().as_ref().unwrap_err(),
            "refused"
        );
        let mut different = request;
        different.take = false;
        assert!(matches!(
            submissions.reserve(id, different),
            LeaseReservation::Conflict
        ));
    }

    #[test]
    fn cancellation_before_arrival_prevents_submission() {
        let submissions = Operations::default();
        let id = Uuid::new_v4();
        assert!(matches!(
            submissions.cancel(super::OperationId(id), "lease"),
            CancellationResult::AlreadyStopped
        ));
        let request = CommandSubmissionRequest {
            operation_id: id.to_string(),
            station_instance_id: Uuid::new_v4().to_string(),
            arm: "left".into(),
            command: StationCommand::Enable,
            lease_token: Some("lease".into()),
            expected_revision: None,
            trace_context: None,
        };
        assert!(matches!(
            submissions.reserve(super::OperationId(id), SubmissionRequest::Single(request)),
            SubmissionReservation::Cancelled
        ));
    }

    #[test]
    fn retries_share_the_original_result_and_different_payloads_conflict() {
        let submissions = Operations::default();
        let id = Uuid::parse_str("11111111-1111-4111-8111-111111111111").unwrap();
        let request = CommandSubmissionRequest {
            operation_id: id.to_string(),
            station_instance_id: "22222222-2222-4222-8222-222222222222".into(),
            arm: "left".into(),
            command: StationCommand::Stop,
            lease_token: None,
            expected_revision: None,
            trace_context: None,
        };
        let SubmissionReservation::New { pending, .. } = submissions.reserve(
            super::OperationId(id),
            SubmissionRequest::Single(request.clone()),
        ) else {
            panic!("first submission must be new");
        };
        let SubmissionReservation::Existing(receiver) = submissions.reserve(
            super::OperationId(id),
            SubmissionRequest::Single(request.clone()),
        ) else {
            panic!("retry must follow the original");
        };
        let outcome = CommandAnswer::Refused {
            reason: RejectionReason::ArmBusy,
            detail: "busy".into(),
        };
        pending.complete(Ok(outcome.clone()));
        assert_eq!(*receiver.borrow(), Some(Ok(outcome.clone())));
        let SubmissionReservation::Existing(later) = submissions.reserve(
            super::OperationId(id),
            SubmissionRequest::Single(request.clone()),
        ) else {
            panic!("completed retry must follow the original");
        };
        assert_eq!(*later.borrow(), Some(Ok(outcome)));
        let mut changed = request;
        changed.arm = "right".into();
        assert!(matches!(
            submissions.reserve(super::OperationId(id), SubmissionRequest::Single(changed)),
            SubmissionReservation::Conflict
        ));
    }

    #[test]
    fn an_abandoned_admission_is_terminal_for_retries_status_and_cancellation() {
        let submissions = Operations::default();
        let id = Uuid::new_v4();
        let operation = super::OperationId(id);
        let request = SubmissionRequest::Single(CommandSubmissionRequest {
            operation_id: id.to_string(),
            station_instance_id: Uuid::new_v4().to_string(),
            arm: "left".into(),
            command: StationCommand::Enable,
            lease_token: Some("lease".into()),
            expected_revision: None,
            trace_context: None,
        });
        let SubmissionReservation::New {
            pending, response, ..
        } = submissions.reserve(operation, request.clone())
        else {
            panic!("first submission must be new");
        };
        drop(pending);
        let Some(hebbian_contracts::OperationStatus::OutcomeUnknown { detail }) =
            submissions.status(operation)
        else {
            panic!("the abandoned operation must be terminal");
        };
        let SubmissionReservation::Existing(retry) = submissions.reserve(operation, request) else {
            panic!("retry must follow the original");
        };
        let failure = super::CommandOutcomeUnknown::AdmissionWorker;
        assert_eq!(detail, failure.to_string());
        assert_eq!(*response.borrow(), Some(Err(failure.clone())));
        assert_eq!(*retry.borrow(), Some(Err(failure)));
        assert!(matches!(
            submissions.cancel(operation, "lease"),
            CancellationResult::AlreadyStopped
        ));
    }

    #[test]
    fn completed_command_outcomes_can_be_evicted_without_reexecuting_old_ids() {
        let submissions = Operations::default();
        let mut requests = Vec::new();
        for _ in 0..=super::MAX_RETAINED_SUBMISSIONS {
            let id = Uuid::new_v4();
            let request = CommandSubmissionRequest {
                operation_id: id.to_string(),
                station_instance_id: Uuid::nil().to_string(),
                arm: "left".into(),
                command: StationCommand::Stop,
                lease_token: None,
                expected_revision: None,
                trace_context: None,
            };
            let SubmissionReservation::New { pending, .. } = submissions.reserve(
                super::OperationId(id),
                SubmissionRequest::Single(request.clone()),
            ) else {
                panic!(
                    "new commands must remain available after completed outcomes fill the cache"
                );
            };
            pending.complete(Err(super::CommandOutcomeUnknown::AdmissionWorker));
            requests.push((id, request));
        }
        assert!(requests.into_iter().any(|(id, request)| matches!(
            submissions.reserve(super::OperationId(id), SubmissionRequest::Single(request)),
            SubmissionReservation::Expired
        )));
    }

    #[test]
    fn an_active_stream_remains_cancellable_when_other_operations_fill_history() {
        let submissions = Operations::default();
        let start_id = Uuid::new_v4();
        let command_id = "the-stream".to_owned();
        let request = |id: Uuid| {
            SubmissionRequest::Single(CommandSubmissionRequest {
                operation_id: id.to_string(),
                station_instance_id: Uuid::nil().to_string(),
                arm: "left".into(),
                command: StationCommand::StreamJoints {
                    joints: vec![0.2],
                    max_joint_speed: None,
                    gripper_opening: None,
                },
                lease_token: Some("lease".into()),
                expected_revision: None,
                trace_context: None,
            })
        };
        let reserve = |id| {
            let SubmissionReservation::New { pending, .. } =
                submissions.reserve(super::OperationId(id), request(id))
            else {
                panic!("stream updates must be admitted when retention has room");
            };
            pending
        };
        let accepted = || {
            Ok(CommandAnswer::Accepted {
                records: vec![hebbian_contracts::ArmRecord {
                    arm: "left".into(),
                    record: hebbian_contracts::CommandRecord::new(
                        command_id.clone(),
                        "stream_joints",
                        hebbian_contracts::UnixNanos(0),
                        hebbian_contracts::RecordedCommand::JointStream {
                            goal_joints: vec![0.0],
                            joint_speed: 1.0,
                        },
                    ),
                }],
            })
        };
        reserve(start_id).complete(accepted());
        let pending: Vec<_> = (1..super::MAX_RETAINED_SUBMISSIONS)
            .map(|_| reserve(Uuid::new_v4()))
            .collect();
        // INVARIANT: With every update still pending, the active stream start
        // would be the only eviction candidate if acceptance made it evictable.
        let next_id = Uuid::new_v4();
        assert!(matches!(
            submissions.reserve(super::OperationId(next_id), request(next_id)),
            SubmissionReservation::Full
        ));
        for admission in pending {
            admission.complete(Err(super::CommandOutcomeUnknown::AdmissionWorker));
        }
        reserve(next_id).complete(accepted());
        let CancellationResult::Requested(CancellationTarget::Commands(records)) =
            submissions.cancel(super::OperationId(start_id), "lease")
        else {
            panic!("the original request must still resolve to the active stream");
        };
        assert_eq!(records[0].record.command_id, command_id);
    }

    #[test]
    fn completed_lease_outcomes_can_be_evicted_without_reacquiring_old_ids() {
        let submissions = LeaseSubmissions::default();
        let mut requests = Vec::new();
        for _ in 0..=super::MAX_RETAINED_SUBMISSIONS {
            let id = Uuid::new_v4();
            let request = hebbian_contracts::LeaseAcquireRequest {
                request_id: id.to_string(),
                station_instance_id: Uuid::nil().to_string(),
                request: hebbian_contracts::LeaseRequest {
                    holder: hebbian_contracts::LeaseHolder::Agent,
                    label: "test".into(),
                    required_cameras: vec![],
                    on_interruption: hebbian_contracts::OnInterruption::WaitForApproval,
                },
                take: false,
            };
            let LeaseReservation::New(sender) = submissions.reserve(id, request.clone()) else {
                panic!("new leases must remain available after completed outcomes fill the cache");
            };
            sender.send_replace(Some(Err("completed".into())));
            requests.push((id, request));
        }
        assert!(requests.into_iter().any(|(id, request)| matches!(
            submissions.reserve(id, request),
            LeaseReservation::Expired
        )));
    }

    #[test]
    fn cancellation_fences_when_all_outcomes_are_in_flight() {
        let submissions = Operations::default();
        let mut pending_admissions = Vec::new();
        let mut request = CommandSubmissionRequest {
            operation_id: String::new(),
            station_instance_id: Uuid::nil().to_string(),
            arm: "left".into(),
            command: StationCommand::Stop,
            lease_token: Some("lease".into()),
            expected_revision: None,
            trace_context: None,
        };
        for _ in 0..super::MAX_RETAINED_SUBMISSIONS {
            let id = Uuid::new_v4();
            request.operation_id = id.to_string();
            let SubmissionReservation::New { pending, .. } = submissions.reserve(
                super::OperationId(id),
                SubmissionRequest::Single(request.clone()),
            ) else {
                panic!("the history has room for this pending admission");
            };
            pending_admissions.push(pending);
        }
        let cancelled_id = Uuid::new_v4();
        request.operation_id = cancelled_id.to_string();
        assert!(matches!(
            submissions.cancel(super::OperationId(cancelled_id), "lease"),
            CancellationResult::AlreadyStopped
        ));
        assert!(matches!(
            submissions.reserve(
                super::OperationId(cancelled_id),
                SubmissionRequest::Single(request)
            ),
            SubmissionReservation::Expired
        ));
    }
}

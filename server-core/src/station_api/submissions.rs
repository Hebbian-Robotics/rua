use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use hebbian_contracts::{CommandAnswer, CommandSubmissionRequest, GroupCommandSubmissionRequest};
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

struct SubmissionHistory<T> {
    retained: HashMap<Uuid, T>,
    seen: HashSet<Uuid>,
}

impl<T> Default for SubmissionHistory<T> {
    fn default() -> Self {
        Self {
            retained: HashMap::new(),
            seen: HashSet::new(),
        }
    }
}

impl<T> SubmissionHistory<T> {
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

    fn insert(&mut self, id: Uuid, submission: T) {
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

#[expect(
    clippy::large_enum_variant,
    reason = "The retained history is bounded. Keep command requests inline to avoid a separate allocation on every submission."
)]
enum Submission {
    Command {
        tracked: TrackedRequest<SubmissionRequest, CommandAnswer>,
        cancelled: watch::Sender<bool>,
    },
    Cancelled,
}

impl Submission {
    fn is_evictable(&self) -> bool {
        match self {
            Self::Command { tracked, .. } => tracked.outcome.borrow().is_some(),
            Self::Cancelled => true,
        }
    }
}

#[derive(Default)]
pub struct CommandSubmissions {
    entries: Mutex<SubmissionHistory<Submission>>,
}

pub enum SubmissionReservation {
    New(
        watch::Sender<Option<Result<CommandAnswer, String>>>,
        watch::Receiver<bool>,
    ),
    Existing(watch::Receiver<Option<Result<CommandAnswer, String>>>),
    Cancelled,
    Conflict,
    Expired,
    Full,
}

pub enum CancellationResult {
    Fenced,
    Pending(watch::Receiver<Option<Result<CommandAnswer, String>>>),
    Expired,
    Conflict,
}

impl CommandSubmissions {
    pub fn reserve(&self, id: Uuid, request: SubmissionRequest) -> SubmissionReservation {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(existing) = entries.retained.get(&id) {
            return match existing {
                Submission::Command { tracked, .. } if tracked.request == request => {
                    SubmissionReservation::Existing(tracked.outcome.subscribe())
                }
                Submission::Command { .. } => SubmissionReservation::Conflict,
                Submission::Cancelled => SubmissionReservation::Cancelled,
            };
        }
        if entries.seen.contains(&id) {
            return SubmissionReservation::Expired;
        }
        if !entries.make_room(Submission::is_evictable) {
            return SubmissionReservation::Full;
        }
        let (outcome, _) = watch::channel(None);
        let (cancelled, cancellation) = watch::channel(false);
        entries.insert(
            id,
            Submission::Command {
                tracked: TrackedRequest {
                    request,
                    outcome: outcome.clone(),
                },
                cancelled,
            },
        );
        SubmissionReservation::New(outcome, cancellation)
    }

    // INVARIANT: Fence the request before reading its admission reply. If it was
    // already enqueued, only the controller records returned for it may be cancelled.
    pub fn cancel(&self, id: Uuid, token: &str, arms: &[String]) -> CancellationResult {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(Submission::Command { tracked, cancelled }) = entries.retained.get(&id) {
            let matches = match &tracked.request {
                SubmissionRequest::Single(request) => {
                    request.lease_token.as_deref() == Some(token)
                        && matches!(arms, [arm] if arm == &request.arm)
                }
                SubmissionRequest::Group(request) => {
                    request.lease_token.as_deref() == Some(token)
                        && arms.len() == request.command.targets.len()
                        && arms
                            .iter()
                            .all(|arm| request.command.targets.contains_key(arm))
                }
            };
            if !matches {
                return CancellationResult::Conflict;
            }
            cancelled.send_replace(true);
            return CancellationResult::Pending(tracked.outcome.subscribe());
        }
        if matches!(entries.retained.get(&id), Some(Submission::Cancelled)) {
            return CancellationResult::Fenced;
        }
        if entries.seen.contains(&id) {
            return CancellationResult::Expired;
        }
        if entries.make_room(Submission::is_evictable) {
            entries.insert(id, Submission::Cancelled);
        } else {
            entries.seen.insert(id);
        }
        CancellationResult::Fenced
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CancellationResult, CommandSubmissions, LeaseReservation, LeaseSubmissions,
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
        let submissions = CommandSubmissions::default();
        let id = Uuid::new_v4();
        let arms = vec!["left".to_owned()];
        assert!(matches!(
            submissions.cancel(id, "lease", &arms),
            CancellationResult::Fenced
        ));
        let request = CommandSubmissionRequest {
            request_id: id.to_string(),
            station_instance_id: Uuid::new_v4().to_string(),
            arm: "left".into(),
            command: StationCommand::Enable,
            lease_token: Some("lease".into()),
            expected_revision: None,
            trace_context: None,
        };
        assert!(matches!(
            submissions.reserve(id, SubmissionRequest::Single(request)),
            SubmissionReservation::Cancelled
        ));
    }

    #[test]
    fn retries_share_the_original_result_and_different_payloads_conflict() {
        let submissions = CommandSubmissions::default();
        let id = Uuid::parse_str("11111111-1111-4111-8111-111111111111").unwrap();
        let request = CommandSubmissionRequest {
            request_id: id.to_string(),
            station_instance_id: "22222222-2222-4222-8222-222222222222".into(),
            arm: "left".into(),
            command: StationCommand::Stop,
            lease_token: None,
            expected_revision: None,
            trace_context: None,
        };
        let SubmissionReservation::New(sender, _) =
            submissions.reserve(id, SubmissionRequest::Single(request.clone()))
        else {
            panic!("first submission must be new");
        };
        let SubmissionReservation::Existing(receiver) =
            submissions.reserve(id, SubmissionRequest::Single(request.clone()))
        else {
            panic!("retry must follow the original");
        };
        let outcome = CommandAnswer::Refused {
            reason: RejectionReason::ArmBusy,
            detail: "busy".into(),
        };
        sender.send_replace(Some(Ok(outcome.clone())));
        assert_eq!(*receiver.borrow(), Some(Ok(outcome.clone())));
        let SubmissionReservation::Existing(later) =
            submissions.reserve(id, SubmissionRequest::Single(request.clone()))
        else {
            panic!("completed retry must follow the original");
        };
        assert_eq!(*later.borrow(), Some(Ok(outcome)));
        let mut changed = request;
        changed.arm = "right".into();
        assert!(matches!(
            submissions.reserve(id, SubmissionRequest::Single(changed)),
            SubmissionReservation::Conflict
        ));
    }

    #[test]
    fn completed_command_outcomes_can_be_evicted_without_reexecuting_old_ids() {
        let submissions = CommandSubmissions::default();
        let mut requests = Vec::new();
        for _ in 0..=super::MAX_RETAINED_SUBMISSIONS {
            let id = Uuid::new_v4();
            let request = CommandSubmissionRequest {
                request_id: id.to_string(),
                station_instance_id: Uuid::nil().to_string(),
                arm: "left".into(),
                command: StationCommand::Stop,
                lease_token: None,
                expected_revision: None,
                trace_context: None,
            };
            let SubmissionReservation::New(sender, _) =
                submissions.reserve(id, SubmissionRequest::Single(request.clone()))
            else {
                panic!(
                    "new commands must remain available after completed outcomes fill the cache"
                );
            };
            sender.send_replace(Some(Err("completed".into())));
            requests.push((id, request));
        }
        assert!(requests.into_iter().any(|(id, request)| matches!(
            submissions.reserve(id, SubmissionRequest::Single(request)),
            SubmissionReservation::Expired
        )));
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
        let submissions = CommandSubmissions::default();
        let mut request = CommandSubmissionRequest {
            request_id: String::new(),
            station_instance_id: Uuid::nil().to_string(),
            arm: "left".into(),
            command: StationCommand::Stop,
            lease_token: Some("lease".into()),
            expected_revision: None,
            trace_context: None,
        };
        for _ in 0..super::MAX_RETAINED_SUBMISSIONS {
            let id = Uuid::new_v4();
            request.request_id = id.to_string();
            assert!(matches!(
                submissions.reserve(id, SubmissionRequest::Single(request.clone())),
                SubmissionReservation::New(..)
            ));
        }
        let cancelled_id = Uuid::new_v4();
        request.request_id = cancelled_id.to_string();
        assert!(matches!(
            submissions.cancel(cancelled_id, "lease", &["left".into()]),
            CancellationResult::Fenced
        ));
        assert!(matches!(
            submissions.reserve(cancelled_id, SubmissionRequest::Single(request)),
            SubmissionReservation::Expired
        ));
    }
}

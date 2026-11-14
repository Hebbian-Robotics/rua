//! Request identity for independently submitted station commands.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use hebbian_contracts::{CommandAnswer, CommandSubmissionRequest, GroupCommandSubmissionRequest};
use tokio::sync::watch;
use uuid::Uuid;

const MAX_RETAINED_SUBMISSIONS: usize = 10_000;

type LeaseOutcome = Result<hebbian_contracts::LeaseGrant, String>;

struct LeaseSubmission {
    request: hebbian_contracts::LeaseAcquireRequest,
    outcome: watch::Sender<Option<LeaseOutcome>>,
}

/// Completed outcomes may be discarded, but their identities remain fenced.
#[derive(Default)]
pub struct LeaseSubmissions {
    entries: Mutex<SubmissionHistory<LeaseSubmission>>,
}

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

pub enum LeaseReservation {
    New(watch::Sender<Option<LeaseOutcome>>),
    Existing(watch::Receiver<Option<LeaseOutcome>>),
    Conflict,
    Expired,
    Full,
}

impl LeaseSubmissions {
    pub fn reserve(
        &self,
        id: Uuid,
        request: hebbian_contracts::LeaseAcquireRequest,
    ) -> LeaseReservation {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(existing) = entries.retained.get(&id) {
            return if existing.request == request {
                LeaseReservation::Existing(existing.outcome.subscribe())
            } else {
                LeaseReservation::Conflict
            };
        }
        if entries.seen.contains(&id) {
            return LeaseReservation::Expired;
        }
        if !entries.make_room(|submission| submission.outcome.borrow().is_some()) {
            return LeaseReservation::Full;
        }
        let (outcome, _) = watch::channel(None);
        entries.insert(
            id,
            LeaseSubmission {
                request,
                outcome: outcome.clone(),
            },
        );
        LeaseReservation::New(outcome)
    }
}

/// A request ID is shared across single-arm and coordinated submissions.
#[derive(Debug, Clone, PartialEq)]
pub enum SubmissionRequest {
    Single(CommandSubmissionRequest),
    Group(GroupCommandSubmissionRequest),
}

struct Submission {
    request: Option<SubmissionRequest>,
    outcome: watch::Sender<Option<Result<CommandAnswer, String>>>,
}

/// Tracks outcomes and permanently fences used identifiers for this process.
#[derive(Default)]
pub struct CommandSubmissions {
    entries: Mutex<SubmissionHistory<Submission>>,
}

/// The caller either owns a new submission or follows an existing one.
pub enum SubmissionReservation {
    New(watch::Sender<Option<Result<CommandAnswer, String>>>),
    Existing(watch::Receiver<Option<Result<CommandAnswer, String>>>),
    Cancelled,
    Conflict,
    Expired,
    Full,
}

pub enum CancellationResult {
    Stopped(hebbian_contracts::LeasedStopOutcome),
    Conflict,
}

impl CommandSubmissions {
    /// Atomically reserve an identifier or follow its original outcome.
    pub fn reserve(&self, id: Uuid, request: SubmissionRequest) -> SubmissionReservation {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(existing) = entries.retained.get(&id) {
            return if existing.request.as_ref() == Some(&request) {
                SubmissionReservation::Existing(existing.outcome.subscribe())
            } else if existing.request.is_none() {
                SubmissionReservation::Cancelled
            } else {
                SubmissionReservation::Conflict
            };
        }
        if entries.seen.contains(&id) {
            return SubmissionReservation::Expired;
        }
        if !entries.make_room(|submission| {
            submission.request.is_none() || submission.outcome.borrow().is_some()
        }) {
            return SubmissionReservation::Full;
        }
        let (outcome, _) = watch::channel(None);
        entries.insert(
            id,
            Submission {
                request: Some(request),
                outcome: outcome.clone(),
            },
        );
        SubmissionReservation::New(outcome)
    }

    /// Fence an identifier and stop its motion under the same reservation lock.
    /// A future arrival for an identifier cancelled before submission is refused.
    pub fn cancel(
        &self,
        id: Uuid,
        token: &str,
        arms: &[String],
        stop: impl FnOnce() -> hebbian_contracts::LeasedStopOutcome,
    ) -> CancellationResult {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(Some(request)) = entries
            .retained
            .get(&id)
            .map(|entry| entry.request.as_ref())
        {
            let matches = match request {
                SubmissionRequest::Single(request) => {
                    request.lease_token.as_deref() == Some(token) && arms == [request.arm.clone()]
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
        }
        let outcome = stop();
        if !matches!(outcome, hebbian_contracts::LeasedStopOutcome::LeaseLost) {
            if !entries.seen.contains(&id)
                && entries.make_room(|submission| {
                    submission.request.is_none() || submission.outcome.borrow().is_some()
                })
            {
                let (sender, _) = watch::channel(None);
                entries.insert(
                    id,
                    Submission {
                        request: None,
                        outcome: sender,
                    },
                );
            } else {
                // Even when all retained submissions are in flight, a stop must
                // never be refused just because its cancellation arrived first.
                entries.seen.insert(id);
            }
        }
        CancellationResult::Stopped(outcome)
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
            submissions.cancel(id, "lease", &arms, || {
                hebbian_contracts::LeasedStopOutcome::Submitted
            }),
            CancellationResult::Stopped(hebbian_contracts::LeasedStopOutcome::Submitted)
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
        let SubmissionReservation::New(sender) =
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
            let SubmissionReservation::New(sender) =
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
    fn cancellation_still_stops_and_fences_when_all_outcomes_are_in_flight() {
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
                SubmissionReservation::New(_)
            ));
        }
        let cancelled_id = Uuid::new_v4();
        request.request_id = cancelled_id.to_string();
        assert!(matches!(
            submissions.cancel(cancelled_id, "lease", &["left".into()], || {
                hebbian_contracts::LeasedStopOutcome::Submitted
            }),
            CancellationResult::Stopped(hebbian_contracts::LeasedStopOutcome::Submitted)
        ));
        assert!(matches!(
            submissions.reserve(cancelled_id, SubmissionRequest::Single(request)),
            SubmissionReservation::Expired
        ));
    }
}

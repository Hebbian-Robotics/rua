//! Adapt the station's admitted arm requests to the controller wire protocol.
//!
//! The station keeps its lease checks. The controller receives only requests
//! that passed them and still admits grouped moves in one control-loop cycle.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::mpsc::{Receiver, Sender};

use hebbian_contracts::{CommandRecord, RejectionReason};

use crate::arm::Rejection;
use crate::controller_transport::{
    ControllerAnswer, ControllerInstanceId, ControllerMessage, ControllerRequest,
    ControllerRequestId,
};
use crate::handoff::{ArmHandle, ArmInbox, ArmRequest};

/// The station side of one arm's handoff. Dropping it drops queued requests.
pub struct StationArm {
    inbox: ArmInbox,
}

impl StationArm {
    pub(crate) fn new(inbox: ArmInbox) -> Self {
        Self { inbox }
    }

    /// Stop admitting requests and reject all requests not yet sent to the controller.
    /// This fence is atomic with enqueue, so a queued request cannot cross reconnect.
    pub fn suspend(&self) {
        self.inbox.set_available(false);
        while let Some((_, answer)) = self.next_request() {
            answer.answer(ControllerAnswer::Refused {
                reason: RejectionReason::NotEnabled,
                detail: "The controller is unavailable; the command was not sent".into(),
            });
        }
    }

    /// Accept new requests after a fresh controller has completed its startup checks.
    pub fn resume(&self) {
        self.inbox.set_available(true);
    }

    /// Take one request without blocking the station or the motor loop.
    #[must_use]
    pub fn next_request(&self) -> Option<(ControllerRequest, PendingStationAnswer)> {
        self.inbox.next().map(|request| match request {
            ArmRequest::ControlHandoffReadiness { reply } => (
                ControllerRequest::ControlHandoffReadiness,
                PendingStationAnswer(PendingStationAnswerKind::Readiness(reply)),
            ),
            ArmRequest::Submit {
                command,
                trace_context,
                reply,
                ..
            } => (
                ControllerRequest::Submit {
                    command,
                    trace_context,
                },
                PendingStationAnswer(PendingStationAnswerKind::Submit(reply)),
            ),
            ArmRequest::MoveArms {
                label,
                moves,
                trace_context,
                reply,
                ..
            } => (
                ControllerRequest::MoveArms {
                    label,
                    moves,
                    trace_context,
                },
                PendingStationAnswer(PendingStationAnswerKind::MoveArms(reply)),
            ),
        })
    }
}

const MAXIMUM_IN_FLIGHT_REQUESTS: usize = 128;

/// The station's handoffs persist across controller instances. Only a fresh,
/// verified controller may activate them; disconnect fences and drains them.
pub struct StationSession {
    arms: BTreeMap<String, StationArm>,
    active: Option<ActiveStationSession>,
    used_instances: HashSet<ControllerInstanceId>,
}

struct ActiveStationSession {
    instance: ControllerInstanceId,
    next_request: u64,
    pending: HashMap<ControllerRequestId, PendingRequest>,
}

struct PendingRequest {
    arms: Vec<String>,
    answer: PendingStationAnswer,
}

impl StationSession {
    #[must_use]
    pub fn new(arms: BTreeMap<String, StationArm>) -> Self {
        for arm in arms.values() {
            arm.suspend();
        }
        Self {
            arms,
            active: None,
            used_instances: HashSet::new(),
        }
    }

    /// Activate only after the controller has completed its startup checks.
    ///
    /// # Errors
    /// A controller session is already active or this instance has been used.
    pub fn activate(&mut self, instance: ControllerInstanceId) -> Result<(), &'static str> {
        if self.active.is_some() {
            return Err("A controller session is already active");
        }
        if !self.used_instances.insert(instance) {
            return Err("A controller instance cannot be reused");
        }
        self.active = Some(ActiveStationSession {
            instance,
            next_request: 0,
            pending: HashMap::new(),
        });
        for arm in self.arms.values() {
            arm.resume();
        }
        Ok(())
    }

    /// Take one request to send. Once taken, losing the connection makes its
    /// execution outcome unknown, even if the transport write did not complete.
    #[must_use]
    pub fn next_message(&mut self) -> Option<ControllerMessage> {
        let active = self.active.as_mut()?;
        if active.pending.len() >= MAXIMUM_IN_FLIGHT_REQUESTS {
            return None;
        }
        let (arm, (request, answer)) = self
            .arms
            .iter()
            .find_map(|(name, arm)| arm.next_request().map(|request| (name, request)))?;
        let request_id = ControllerRequestId(active.next_request);
        active.next_request = active
            .next_request
            .checked_add(1)
            .expect("controller request IDs exhausted");
        let record_arms = match &request {
            ControllerRequest::MoveArms { moves, .. } => {
                moves.iter().map(|part| part.arm.clone()).collect()
            }
            _ => vec![arm.clone()],
        };
        active.pending.insert(
            request_id,
            PendingRequest {
                arms: record_arms,
                answer,
            },
        );
        Some(ControllerMessage::Request {
            request_id,
            arm: arm.clone(),
            request,
        })
    }

    /// Route an answer to its waiter. Returns false for unknown request IDs or
    /// messages that are not answers, so the connection can be treated as invalid.
    pub fn answer(
        &mut self,
        instance: ControllerInstanceId,
        request_id: ControllerRequestId,
        answer: ControllerAnswer,
        mut remember_record: impl FnMut(&str, &CommandRecord),
    ) -> bool {
        let Some(waiter) = self
            .active
            .as_mut()
            .filter(|active| active.instance == instance)
            .and_then(|active| active.pending.remove(&request_id))
        else {
            return false;
        };
        match (&waiter.answer.0, &answer) {
            (PendingStationAnswerKind::Submit(_), ControllerAnswer::Submitted(record))
                if waiter.arms.len() == 1 =>
            {
                remember_record(&waiter.arms[0], record);
            }
            (PendingStationAnswerKind::MoveArms(_), ControllerAnswer::MovedArms(records))
                if records.len() == waiter.arms.len() =>
            {
                for (arm, record) in waiter.arms.iter().zip(records) {
                    remember_record(arm, record);
                }
            }
            (
                PendingStationAnswerKind::Readiness(_),
                ControllerAnswer::ControlHandoffReadiness(_),
            )
            | (_, ControllerAnswer::Refused { .. }) => {}
            _ => {
                waiter.answer.answer(ControllerAnswer::Refused {
                    reason: RejectionReason::Malformed,
                    detail: "Controller returned the wrong number of command records".into(),
                });
                return false;
            }
        }
        waiter.answer.answer(answer);
        true
    }

    /// Fence the handoffs, reject unsent work, and report sent work as unknown.
    pub fn disconnect(&mut self) {
        for arm in self.arms.values() {
            arm.suspend();
        }
        if let Some(active) = self.active.take() {
            for pending in active.pending.into_values() {
                pending.answer.answer(ControllerAnswer::Refused {
                    reason: RejectionReason::NotEnabled,
                    detail: "The controller connection was lost; the command outcome is unknown"
                        .into(),
                });
            }
        }
    }
}

impl Drop for StationSession {
    fn drop(&mut self) {
        self.disconnect();
    }
}

/// The local waiter corresponding to a request sent over the process boundary.
/// Dropping this value wakes the waiter with the existing disconnected outcome.
pub struct PendingStationAnswer(PendingStationAnswerKind);

enum PendingStationAnswerKind {
    Readiness(Sender<Result<bool, Rejection>>),
    Submit(crate::handoff::CommandReply<Result<CommandRecord, Rejection>>),
    MoveArms(crate::handoff::CommandReply<Result<Vec<CommandRecord>, Rejection>>),
}

impl PendingStationAnswer {
    /// Deliver an answer to the waiting station request. An answer of the wrong
    /// kind is refused instead of being attributed to a different operation.
    pub fn answer(self, answer: ControllerAnswer) {
        let refusal = |reason, detail| Rejection { reason, detail };
        match (self.0, answer) {
            (
                PendingStationAnswerKind::Readiness(reply),
                ControllerAnswer::ControlHandoffReadiness(ready),
            ) => {
                let _ = reply.send(Ok(ready));
            }
            (PendingStationAnswerKind::Submit(reply), ControllerAnswer::Submitted(record)) => {
                reply.send(Ok(*record));
            }
            (PendingStationAnswerKind::MoveArms(reply), ControllerAnswer::MovedArms(records)) => {
                reply.send(Ok(records));
            }
            (pending, ControllerAnswer::Refused { reason, detail }) => {
                pending.refuse(refusal(reason, detail));
            }
            (pending, _) => pending.refuse(refusal(
                RejectionReason::Malformed,
                "Controller answered with the wrong outcome type".into(),
            )),
        }
    }
}

impl PendingStationAnswerKind {
    fn refuse(self, refusal: Rejection) {
        match self {
            Self::Readiness(reply) => {
                let _ = reply.send(Err(refusal));
            }
            Self::Submit(reply) => {
                reply.send(Err(refusal));
            }
            Self::MoveArms(reply) => {
                reply.send(Err(refusal));
            }
        }
    }
}

/// The controller side of the handoff, connected directly to its control loop.
#[derive(Clone)]
pub struct LoopArm {
    handle: ArmHandle,
}

impl LoopArm {
    pub(crate) fn new(handle: ArmHandle) -> Self {
        Self { handle }
    }

    /// Queue one station-admitted request. The returned waiter is for a
    /// transport thread; the control loop must never block on it.
    ///
    /// # Errors
    /// The local loop no longer accepts requests.
    pub fn submit(
        &self,
        request: ControllerRequest,
    ) -> Result<PendingLoopAnswer, ControllerAnswer> {
        let pending = match request {
            ControllerRequest::ControlHandoffReadiness => self
                .handle
                .control_handoff_readiness()
                .map(PendingLoopAnswerKind::Readiness),
            ControllerRequest::Submit {
                command,
                trace_context,
            } => self
                .handle
                .submit_unawaited(command, trace_context)
                .map(PendingLoopAnswerKind::Submit),
            ControllerRequest::MoveArms {
                label,
                moves,
                trace_context,
            } => self
                .handle
                .move_arms_unawaited(label, moves, trace_context)
                .map(PendingLoopAnswerKind::MoveArms),
        };
        pending
            .map(PendingLoopAnswer)
            .map_err(|rejection| ControllerAnswer::Refused {
                reason: rejection.reason,
                detail: rejection.detail,
            })
    }
}

/// A controller request awaiting the loop's decision.
pub struct PendingLoopAnswer(PendingLoopAnswerKind);

enum PendingLoopAnswerKind {
    Readiness(Receiver<Result<bool, Rejection>>),
    Submit(Receiver<Result<CommandRecord, Rejection>>),
    MoveArms(Receiver<Result<Vec<CommandRecord>, Rejection>>),
}

impl PendingLoopAnswer {
    /// Wait for the loop on a transport thread.
    #[must_use]
    pub fn wait(self) -> ControllerAnswer {
        fn convert<T>(
            result: Result<Result<T, Rejection>, std::sync::mpsc::RecvError>,
            accepted: impl FnOnce(T) -> ControllerAnswer,
        ) -> ControllerAnswer {
            match result {
                Ok(Ok(value)) => accepted(value),
                Ok(Err(rejection)) => ControllerAnswer::Refused {
                    reason: rejection.reason,
                    detail: rejection.detail,
                },
                Err(_) => ControllerAnswer::Refused {
                    reason: RejectionReason::NotEnabled,
                    detail:
                        "The control loop stopped before answering; the command outcome is unknown"
                            .into(),
                },
            }
        }
        match self.0 {
            PendingLoopAnswerKind::Readiness(receiver) => {
                convert(receiver.recv(), ControllerAnswer::ControlHandoffReadiness)
            }
            PendingLoopAnswerKind::Submit(receiver) => convert(receiver.recv(), |record| {
                ControllerAnswer::Submitted(Box::new(record))
            }),
            PendingLoopAnswerKind::MoveArms(receiver) => {
                convert(receiver.recv(), ControllerAnswer::MovedArms)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use hebbian_contracts::Command;

    use super::*;
    use crate::arm::ArmCore;
    use crate::config::{ExecutorConfig, TEST_CONFIG};
    use crate::handoff::{apply_request, arm_channel};

    #[test]
    fn an_admitted_command_reaches_the_loop_and_its_record_returns_to_the_station() {
        let (station_handle, station_inbox) = arm_channel();
        let (loop_handle, loop_inbox) = arm_channel();
        let station_arm = StationArm::new(station_inbox);
        let loop_arm = LoopArm::new(loop_handle);
        let waiting = station_handle
            .submit_unawaited(Command::Enable, None)
            .unwrap();
        let (request, station_answer) = station_arm.next_request().unwrap();
        let loop_answer = loop_arm.submit(request).unwrap();
        let config = ExecutorConfig::parse(TEST_CONFIG).unwrap();
        let mut core = ArmCore::new(config.arm_limits(), |_| {});
        apply_request(
            loop_inbox.next().unwrap(),
            "arm",
            &mut BTreeMap::from([("arm", &mut core)]),
        );
        station_answer.answer(loop_answer.wait());
        assert_eq!(waiting.recv().unwrap().unwrap().label, "enable");
    }

    #[test]
    fn losing_a_controller_wakes_the_waiter_without_accepting_a_command() {
        let (station_handle, station_inbox) = arm_channel();
        let station_arm = StationArm::new(station_inbox);
        let waiting = station_handle
            .submit_unawaited(Command::Enable, None)
            .unwrap();
        let (_, pending_answer) = station_arm.next_request().unwrap();
        drop(pending_answer);
        assert!(waiting.recv().is_err());
    }

    #[test]
    fn a_disconnected_controller_rejects_queued_and_new_commands_before_reconnect() {
        let (station_handle, station_inbox) = arm_channel();
        let station_arm = StationArm::new(station_inbox);
        let queued = station_handle
            .submit_unawaited(Command::Enable, None)
            .unwrap();
        station_arm.suspend();
        assert!(queued.recv().unwrap().is_err());
        assert!(
            station_handle
                .submit_unawaited(Command::Enable, None)
                .is_err()
        );
        station_arm.resume();
        assert!(station_arm.next_request().is_none());
        assert!(
            station_handle
                .submit_unawaited(Command::Enable, None)
                .is_ok()
        );
    }

    #[test]
    fn reconnect_fences_unsent_commands_and_rejects_old_instance_answers() {
        let (handle, inbox) = arm_channel();
        let mut session =
            StationSession::new(BTreeMap::from([("arm".into(), StationArm::new(inbox))]));
        let first = ControllerInstanceId([1; 16]);
        let second = ControllerInstanceId([2; 16]);
        session.activate(first).unwrap();
        let sent = handle.submit_unawaited(Command::Enable, None).unwrap();
        let ControllerMessage::Request { request_id, .. } = session.next_message().unwrap() else {
            panic!("expected a request")
        };
        let unsent = handle.submit_unawaited(Command::Disable, None).unwrap();
        session.disconnect();
        assert!(sent.recv().unwrap().unwrap_err().detail.contains("unknown"));
        assert!(
            unsent
                .recv()
                .unwrap()
                .unwrap_err()
                .detail
                .contains("not sent")
        );
        assert!(handle.submit_unawaited(Command::Enable, None).is_err());
        assert!(session.activate(first).is_err());
        session.activate(second).unwrap();
        let next = handle.submit_unawaited(Command::Enable, None).unwrap();
        let ControllerMessage::Request {
            request_id: next_id,
            ..
        } = session.next_message().unwrap()
        else {
            panic!("expected a request")
        };
        assert_eq!(request_id, next_id);
        assert!(!session.answer(
            first,
            request_id,
            ControllerAnswer::ControlHandoffReadiness(true),
            |_, _| {}
        ));
        assert!(next.try_recv().is_err());
        session.disconnect();
        assert!(next.recv().unwrap().unwrap_err().detail.contains("unknown"));
    }
}

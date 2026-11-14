//! How a client's request reaches an arm's state machine without a lock.
//!
//! The control loop owns every arm's [`ArmCore`]. A reader thread never touches
//! one: it sends an [`ArmRequest`] down its arm's channel and waits for the
//! answer. At the top of each cycle the loop takes what has arrived (a bounded
//! number of requests, never waiting), applies each to the cores it owns and
//! sends the answer back. So the real-time thread shares no lock with any other
//! thread: a channel send and a try-receive never block on a thread that holds
//! something.

use std::collections::BTreeMap;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use crate::arm::{ArmCore, GroupMembership, Now, Rejection, ShortOfTarget};
use crate::sync::LockUnpoisoned;
use hebbian_contracts::{ArmMove, Command, CommandRecord, RejectionReason, TraceContext};

/// Requests the loop applies per arm per cycle; a flood of requests waits for
/// the next cycles rather than stretching this one.
pub(crate) const MAXIMUM_REQUESTS_PER_CYCLE: usize = 64;

/// A control-loop reply can be consumed by a blocking or asynchronous caller.
pub(crate) enum CommandReply<T> {
    Blocking(Sender<T>),
    Async(tokio::sync::oneshot::Sender<T>),
}

impl<T> CommandReply<T> {
    pub(crate) fn send(self, value: T) {
        match self {
            Self::Blocking(sender) => {
                let _ = sender.send(value);
            }
            Self::Async(sender) => {
                let _ = sender.send(value);
            }
        }
    }
}

/// What a reader asks of the loop, with the way to answer.
pub(crate) enum ArmRequest {
    ControlHandoffReadiness {
        reply: Sender<Result<bool, Rejection>>,
    },
    Submit {
        command: Command,
        trace_context: Option<TraceContext>,
        now: Now,
        reply: CommandReply<Result<CommandRecord, Rejection>>,
    },
    MoveArms {
        label: String,
        moves: Vec<ArmMove>,
        trace_context: Option<TraceContext>,
        now: Now,
        reply: CommandReply<Result<Vec<CommandRecord>, Rejection>>,
    },
}

/// A reader's end: sends requests to one arm's loop and waits for each answer.
#[derive(Clone)]
pub(crate) struct ArmHandle {
    requests: Sender<ArmRequest>,
    available: Arc<Mutex<bool>>,
}

/// The loop's end: the requests its arm's readers have sent.
pub(crate) struct ArmInbox {
    requests: Receiver<ArmRequest>,
    available: Arc<Mutex<bool>>,
}

#[must_use]
pub(crate) fn arm_channel() -> (ArmHandle, ArmInbox) {
    let (requests, received) = channel();
    let available = Arc::new(Mutex::new(true));
    (
        ArmHandle {
            requests,
            available: Arc::clone(&available),
        },
        ArmInbox {
            requests: received,
            available,
        },
    )
}

pub(crate) fn loop_not_running() -> Rejection {
    Rejection {
        reason: RejectionReason::NotEnabled,
        detail: "The control loop is not running".into(),
    }
}

impl ArmHandle {
    fn enqueue(&self, request: ArmRequest) -> Result<(), Rejection> {
        let available = self.available.lock_unpoisoned();
        if !*available {
            return Err(loop_not_running());
        }
        self.requests.send(request).map_err(|_| loop_not_running())
    }

    pub(crate) fn control_handoff_readiness(
        &self,
    ) -> Result<Receiver<Result<bool, Rejection>>, Rejection> {
        let (reply, answer) = channel();
        self.enqueue(ArmRequest::ControlHandoffReadiness { reply })?;
        Ok(answer)
    }

    /// Send a command to the arm without waiting; the loop answers on the
    /// returned channel at the start of a cycle.
    pub(crate) fn submit_unawaited(
        &self,
        command: Command,
        trace_context: Option<TraceContext>,
    ) -> Result<Receiver<Result<CommandRecord, Rejection>>, Rejection> {
        let (reply, answer) = channel();
        self.enqueue(ArmRequest::Submit {
            command,
            trace_context,
            now: Now::current(),
            reply: CommandReply::Blocking(reply),
        })?;
        Ok(answer)
    }

    /// Send a command whose control-loop admission can be awaited asynchronously.
    pub(crate) fn submit_async(
        &self,
        command: Command,
        trace_context: Option<TraceContext>,
    ) -> Result<tokio::sync::oneshot::Receiver<Result<CommandRecord, Rejection>>, Rejection> {
        let (reply, answer) = tokio::sync::oneshot::channel();
        self.enqueue(ArmRequest::Submit {
            command,
            trace_context,
            now: Now::current(),
            reply: CommandReply::Async(reply),
        })?;
        Ok(answer)
    }

    /// Submit an atomic group move with an asynchronously awaitable reply.
    pub(crate) fn move_arms_async(
        &self,
        label: String,
        moves: Vec<ArmMove>,
        trace_context: Option<TraceContext>,
    ) -> Result<tokio::sync::oneshot::Receiver<Result<Vec<CommandRecord>, Rejection>>, Rejection>
    {
        let (reply, answer) = tokio::sync::oneshot::channel();
        self.enqueue(ArmRequest::MoveArms {
            label,
            moves,
            trace_context,
            now: Now::current(),
            reply: CommandReply::Async(reply),
        })?;
        Ok(answer)
    }

    /// Admit a grouped move into every arm it names, or none.
    pub(crate) fn move_arms_unawaited(
        &self,
        label: String,
        moves: Vec<ArmMove>,
        trace_context: Option<TraceContext>,
    ) -> Result<Receiver<Result<Vec<CommandRecord>, Rejection>>, Rejection> {
        let (reply, answer) = channel();
        self.enqueue(ArmRequest::MoveArms {
            label,
            moves,
            trace_context,
            now: Now::current(),
            reply: CommandReply::Blocking(reply),
        })?;
        Ok(answer)
    }
}

impl ArmInbox {
    /// Fence new requests before draining a disconnected controller's queue.
    pub(crate) fn set_available(&self, available: bool) {
        *self.available.lock_unpoisoned() = available;
    }

    /// The next request that has already arrived, if any; never waits.
    pub(crate) fn next(&self) -> Option<ArmRequest> {
        self.requests.try_recv().ok()
    }
}

/// Apply one request that came to the arm called `own`; `cores` holds every arm
/// the loop owns, by name, `own` included.
pub(crate) fn apply_request(
    request: ArmRequest,
    own: &str,
    cores: &mut BTreeMap<&str, &mut ArmCore>,
) {
    match request {
        ArmRequest::ControlHandoffReadiness { reply } => {
            let answer = cores
                .get(own)
                .ok_or_else(loop_not_running)
                .and_then(|core| core.ready_for_control_handoff());
            let _ = reply.send(answer);
        }
        ArmRequest::Submit {
            command,
            trace_context,
            now,
            reply,
        } => {
            let answer = match cores.get_mut(own) {
                Some(core) => {
                    core.set_trace_context(trace_context);
                    let answer = core.submit(command, now);
                    core.set_trace_context(None);
                    answer
                }
                None => Err(loop_not_running()),
            };
            reply.send(answer);
        }
        ArmRequest::MoveArms {
            label,
            moves,
            trace_context,
            now,
            reply,
        } => {
            for core in cores.values_mut() {
                core.set_trace_context(trace_context.clone());
            }
            let answer = admit_group(cores, &label, moves, now);
            for core in cores.values_mut() {
                core.set_trace_context(None);
            }
            reply.send(answer);
        }
    }
}

/// Admit a grouped move into every named arm, or into none.
pub(crate) fn admit_group(
    cores: &mut BTreeMap<&str, &mut ArmCore>,
    label: &str,
    moves: Vec<ArmMove>,
    now: Now,
) -> Result<Vec<CommandRecord>, Rejection> {
    let invalid = |detail: String| Rejection {
        reason: RejectionReason::InvalidTarget,
        detail,
    };
    if moves.is_empty() {
        return Err(invalid("A grouped move needs at least one arm".into()));
    }
    let mut names: Vec<String> = moves.iter().map(|part| part.arm.clone()).collect();
    names.sort_unstable();
    if let Some([duplicate, _]) = names.windows(2).find(|pair| pair[0] == pair[1]) {
        return Err(invalid(format!("Arm {duplicate} appears twice")));
    }
    for part in &moves {
        let [segment] = part.plan.segments.as_slice() else {
            return Err(invalid(format!(
                "{}: a grouped move is one straight run per arm",
                part.arm
            )));
        };
        if segment.corners.len() != 2 {
            return Err(invalid(format!(
                "{}: a grouped move is one straight run per arm",
                part.arm
            )));
        }
    }
    for name in &names {
        if !cores.contains_key(name.as_str()) {
            return Err(invalid(format!("No arm {name}")));
        }
    }
    // The loop is the only thread that touches a core, so the checks below
    // still hold when the moves are submitted.
    for part in &moves {
        cores[part.arm.as_str()]
            .check_move(&part.plan, part.joint_speed)
            .map_err(|refused| Rejection {
                reason: refused.reason,
                detail: format!("{}: {}", part.arm, refused.detail),
            })?;
    }
    let group = GroupMembership {
        id: crate::arm::new_identifier(),
        arms: moves.iter().map(|part| part.arm.clone()).collect(),
    };
    let mut records = Vec::new();
    for part in moves {
        let core = cores
            .get_mut(part.arm.as_str())
            .expect("every named arm was found");
        let record = core.submit_move(
            label,
            part.plan,
            part.joint_speed,
            ShortOfTarget {
                path_limited: part.path_limited,
                pose_limited: part.pose_limited,
            },
            Some(group.clone()),
            now,
        );
        match record {
            Ok(record) => records.push(record),
            Err(refused) => {
                // Every check passed, so this is not expected; do not leave
                // part of a group queued.
                for name in &names {
                    if let Some(core) = cores.get_mut(name.as_str()) {
                        let _ = core.submit(Command::Stop, now);
                    }
                }
                return Err(refused);
            }
        }
    }
    Ok(records)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arm::Limits;
    use hebbian_contracts::{CornerPlan, Segment};

    fn core() -> ArmCore {
        ArmCore::new(
            Limits {
                joint_count: 2,
                lower: vec![-2.0, -2.0],
                upper: vec![2.0, 2.0],
                joint_limit_recovery_tolerance_radians: 0.035,
                gripper: None,
                pose_goals: None,
                jaw_closing_axis: hebbian_contracts::JawClosingAxis::X,
                default_joint_speed: 1.0,
                max_joint_speed: 2.0,
            },
            |_| {},
        )
    }

    fn holding_core() -> ArmCore {
        let mut core = core();
        core.enabled(&[0.0, 0.0], Now::current());
        core
    }

    fn part(arm: &str, target: [f64; 2]) -> ArmMove {
        ArmMove {
            arm: arm.to_owned(),
            plan: CornerPlan {
                segments: vec![Segment {
                    label: "go".into(),
                    corners: vec![vec![0.0, 0.0], target.to_vec()],
                    minimum_duration_seconds: None,
                    target_position: None,
                    target_quaternion: None,
                }],
                peak_speed: 1.0,
            },
            joint_speed: 1.0,
            path_limited: None,
            pose_limited: None,
        }
    }

    #[test]
    fn an_empty_inbox_returns_at_once_and_a_request_waits_for_the_loop_to_answer() {
        let (handle, inbox) = arm_channel();
        assert!(inbox.next().is_none(), "an empty inbox does not wait");

        let answer = handle.submit_unawaited(Command::Enable, None).unwrap();
        assert!(answer.try_recv().is_err());
        let request = loop {
            if let Some(request) = inbox.next() {
                break request;
            }
            std::thread::yield_now();
        };
        let mut owned = core();
        let mut cores = BTreeMap::from([("arm", &mut owned)]);
        apply_request(request, "arm", &mut cores);
        let record = answer.recv().unwrap().unwrap();
        assert_eq!(record.label, "enable");
        assert_eq!(
            owned.record(&record.command_id).map(|held| &held.label),
            Some(&record.label),
            "the loop's core holds the record the reader was given"
        );
    }

    #[test]
    fn a_refusal_reaches_the_reader_as_it_came_from_the_core() {
        let (handle, inbox) = arm_channel();
        let answer = handle.submit_unawaited(Command::Disable, None).unwrap();
        let mut owned = core();
        let mut cores = BTreeMap::from([("arm", &mut owned)]);
        apply_request(inbox.next().unwrap(), "arm", &mut cores);
        let refused = answer.recv().unwrap().unwrap_err();
        assert_eq!(refused.reason, RejectionReason::NotEnabled);
    }

    #[test]
    fn a_reader_is_refused_not_stranded_when_the_loop_is_gone() {
        let (handle, inbox) = arm_channel();
        let waiting = handle.submit_unawaited(Command::Enable, None).unwrap();
        drop(inbox);
        assert!(waiting.recv().is_err(), "the unanswered request is dropped");
        assert_eq!(
            handle
                .submit_unawaited(Command::Enable, None)
                .unwrap_err()
                .detail,
            "The control loop is not running"
        );
    }

    #[test]
    fn requests_are_applied_in_the_order_they_were_sent() {
        let (handle, inbox) = arm_channel();
        let first = handle.submit_unawaited(Command::Enable, None).unwrap();
        let second = handle.submit_unawaited(Command::Enable, None).unwrap();
        let mut owned = core();
        let mut cores = BTreeMap::from([("arm", &mut owned)]);
        while let Some(request) = inbox.next() {
            apply_request(request, "arm", &mut cores);
        }
        assert!(first.recv().unwrap().is_ok());
        assert_eq!(
            second.recv().unwrap().unwrap_err().reason,
            RejectionReason::ArmBusy,
            "the second enable found the first queued"
        );
    }

    #[test]
    fn a_group_with_one_arm_that_refuses_is_queued_in_none() {
        let mut left = holding_core();
        let mut right = core();
        let mut cores = BTreeMap::from([("left", &mut left), ("right", &mut right)]);
        let refused = admit_group(
            &mut cores,
            "together",
            vec![part("left", [0.5, 0.0]), part("right", [0.5, 0.0])],
            Now::current(),
        )
        .unwrap_err();
        assert!(refused.detail.starts_with("right: "), "{}", refused.detail);
        drop(cores);
        assert!(
            left.next_job().is_none(),
            "the arm that could take its part was left alone"
        );
    }

    #[test]
    fn a_group_goes_into_every_arm_with_one_shared_id() {
        let mut left = holding_core();
        let mut right = holding_core();
        let mut cores = BTreeMap::from([("left", &mut left), ("right", &mut right)]);
        let records = admit_group(
            &mut cores,
            "together",
            vec![part("left", [0.5, 0.0]), part("right", [0.0, 0.5])],
            Now::current(),
        )
        .unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].group_id, records[1].group_id);
        assert!(records[0].group_id.is_some());
        drop(cores);
        assert!(left.next_job().is_some() && right.next_job().is_some());
    }

    #[test]
    fn a_group_is_refused_for_an_unknown_or_repeated_arm_or_no_arm() {
        let mut only = holding_core();
        let mut cores = BTreeMap::from([("only", &mut only)]);
        let mut detail = |moves| {
            admit_group(&mut cores, "x", moves, Now::current())
                .unwrap_err()
                .detail
        };
        assert_eq!(detail(vec![]), "A grouped move needs at least one arm");
        assert_eq!(
            detail(vec![part("only", [0.1, 0.0]), part("only", [0.1, 0.0])]),
            "Arm only appears twice"
        );
        assert_eq!(detail(vec![part("gone", [0.1, 0.0])]), "No arm gone");
    }
}

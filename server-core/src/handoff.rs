// INVARIANT: Only the control loop mutates ArmCore. Readers send requests through per-arm channels; each cycle drains a bounded number without waiting on readers.

use std::collections::BTreeMap;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use crate::action_step::{PreparedArmAction, PreparedMovement};
use crate::arm::{
    ArmCore, GripperTarget, GroupMembership, MotionScope, MoveDecision, Now, ReachGoal, Rejection,
    ShortOfTarget,
};
use crate::sync::LockUnpoisoned;
#[cfg(test)]
use hebbian_contracts::ArmMove;
use hebbian_contracts::{ArmRecord, Command, CommandRecord, RejectionReason, TraceContext};

// CONSTRAINT: Bound per-arm admissions per cycle so a request flood cannot stretch the control deadline.
pub(crate) const MAXIMUM_REQUESTS_PER_CYCLE: usize = 64;

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

pub(crate) enum ArmRequest {
    Single(SingleArmRequest),
    Group(GroupRequest),
}

pub(crate) enum SingleArmRequest {
    ControlHandoffReadiness {
        reply: Sender<Result<bool, Rejection>>,
    },
    CancelCommand {
        command_id: String,
        reply: Sender<Result<(), Rejection>>,
    },
    Submit {
        command: Command,
        trace_context: Option<TraceContext>,
        now: Now,
        reply: CommandReply<Result<CommandRecord, Rejection>>,
    },
}

pub(crate) enum GroupRequest {
    ActionStep {
        actions: Vec<PreparedArmAction>,
        trace_context: Option<TraceContext>,
        now: Now,
        reply: CommandReply<Result<Vec<ArmRecord>, Rejection>>,
    },
}

#[derive(Clone)]
pub(crate) struct ArmHandle {
    requests: Sender<ArmRequest>,
    available: Arc<Mutex<bool>>,
}

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
        self.enqueue(ArmRequest::Single(
            SingleArmRequest::ControlHandoffReadiness { reply },
        ))?;
        Ok(answer)
    }

    pub(crate) fn cancel_command(
        &self,
        command_id: String,
    ) -> Result<Receiver<Result<(), Rejection>>, Rejection> {
        let (reply, answer) = channel();
        self.enqueue(ArmRequest::Single(SingleArmRequest::CancelCommand {
            command_id,
            reply,
        }))?;
        Ok(answer)
    }

    pub(crate) fn submit_unawaited(
        &self,
        command: Command,
        trace_context: Option<TraceContext>,
    ) -> Result<Receiver<Result<CommandRecord, Rejection>>, Rejection> {
        let (reply, answer) = channel();
        self.enqueue(ArmRequest::Single(SingleArmRequest::Submit {
            command,
            trace_context,
            now: Now::current(),
            reply: CommandReply::Blocking(reply),
        }))?;
        Ok(answer)
    }

    pub(crate) fn submit_async(
        &self,
        command: Command,
        trace_context: Option<TraceContext>,
    ) -> Result<tokio::sync::oneshot::Receiver<Result<CommandRecord, Rejection>>, Rejection> {
        let (reply, answer) = tokio::sync::oneshot::channel();
        self.enqueue(ArmRequest::Single(SingleArmRequest::Submit {
            command,
            trace_context,
            now: Now::current(),
            reply: CommandReply::Async(reply),
        }))?;
        Ok(answer)
    }

    pub(crate) fn action_step_async(
        &self,
        actions: Vec<PreparedArmAction>,
        trace_context: Option<TraceContext>,
    ) -> Result<tokio::sync::oneshot::Receiver<Result<Vec<ArmRecord>, Rejection>>, Rejection> {
        let (reply, answer) = tokio::sync::oneshot::channel();
        self.enqueue(ArmRequest::Group(GroupRequest::ActionStep {
            actions,
            trace_context,
            now: Now::current(),
            reply: CommandReply::Async(reply),
        }))?;
        Ok(answer)
    }

    pub(crate) fn action_step_unawaited(
        &self,
        actions: Vec<PreparedArmAction>,
        trace_context: Option<TraceContext>,
    ) -> Result<Receiver<Result<Vec<ArmRecord>, Rejection>>, Rejection> {
        let (reply, answer) = channel();
        self.enqueue(ArmRequest::Group(GroupRequest::ActionStep {
            actions,
            trace_context,
            now: Now::current(),
            reply: CommandReply::Blocking(reply),
        }))?;
        Ok(answer)
    }
}

impl ArmInbox {
    // INVARIANT: Fence new requests before draining a disconnected controller's queue.
    pub(crate) fn set_available(&self, available: bool) {
        *self.available.lock_unpoisoned() = available;
    }

    pub(crate) fn next(&self) -> Option<ArmRequest> {
        self.requests.try_recv().ok()
    }
}

impl SingleArmRequest {
    pub(crate) fn apply(self, arm: &mut ArmCore) {
        match self {
            Self::ControlHandoffReadiness { reply } => {
                let _ = reply.send(arm.ready_for_control_handoff());
            }
            Self::CancelCommand { command_id, reply } => {
                arm.cancel_command(&command_id, Now::current());
                let _ = reply.send(Ok(()));
            }
            Self::Submit {
                command,
                trace_context,
                now,
                reply,
            } => {
                arm.set_trace_context(trace_context);
                let answer = arm.submit(command, now);
                arm.set_trace_context(None);
                reply.send(answer);
            }
        }
    }
}

impl GroupRequest {
    pub(crate) fn apply(self, cores: &mut BTreeMap<&str, &mut ArmCore>) {
        match self {
            Self::ActionStep {
                actions,
                trace_context,
                now,
                reply,
            } => {
                for core in cores.values_mut() {
                    core.set_trace_context(trace_context.clone());
                }
                let answer = admit_action_step(cores, actions, now);
                for core in cores.values_mut() {
                    core.set_trace_context(None);
                }
                reply.send(answer);
            }
        }
    }
}

#[cfg(test)]
pub(crate) fn admit_group(
    cores: &mut BTreeMap<&str, &mut ArmCore>,
    label: &str,
    moves: Vec<ArmMove>,
    now: Now,
) -> Result<Vec<CommandRecord>, Rejection> {
    admit_action_step(
        cores,
        moves
            .into_iter()
            .map(|part| PreparedArmAction::from_group_move(label, part))
            .collect(),
        now,
    )
    .map(|records| records.into_iter().map(|record| record.record).collect())
}

enum AdmittedMovement {
    Trajectory {
        label: String,
        decision: MoveDecision,
        short: ShortOfTarget,
    },
    PolicyPose {
        label: String,
        goal: ReachGoal,
    },
}

struct AdmittedArmAction {
    arm: String,
    movement: Option<AdmittedMovement>,
    gripper: Option<GripperTarget>,
}

// INVARIANT: Decide the whole arm action without changing records, targets or events.
fn prepare_arm_action(
    core: &ArmCore,
    action: PreparedArmAction,
    motion_scope: MotionScope,
) -> Result<AdmittedArmAction, Rejection> {
    let (arm, movement, gripper) = action.into_parts();
    let movement = movement
        .map(|movement| -> Result<_, Rejection> {
            Ok(match movement {
                PreparedMovement::Trajectory {
                    label,
                    plan,
                    joint_speed,
                    path_limited,
                    pose_limited,
                } => AdmittedMovement::Trajectory {
                    label,
                    decision: MoveDecision::prepare(core, plan, joint_speed, motion_scope)?,
                    short: ShortOfTarget {
                        path_limited,
                        pose_limited,
                    },
                },
                PreparedMovement::PolicyPose {
                    label,
                    position,
                    orientation,
                } => AdmittedMovement::PolicyPose {
                    label,
                    goal: core.check_reach(position, orientation)?,
                },
            })
        })
        .transpose()?;
    let gripper = gripper
        .map(|gripper| core.check_gripper_target(gripper.angle, gripper.joint_speed))
        .transpose()?;
    Ok(AdmittedArmAction {
        arm,
        movement,
        gripper,
    })
}

fn check_group_clearance(
    cores: &BTreeMap<&str, &mut ArmCore>,
    actions: &[AdmittedArmAction],
) -> anyhow::Result<()> {
    use hebbian_robot_geometry::self_collision::CollisionWorkspace;

    const INTER_ARM_PENETRATION_METERS: f64 = 0.003;
    if actions.len() < 2 {
        return Ok(());
    }
    let mut members = Vec::with_capacity(actions.len());
    let mut moving_arms = 0;
    let mut moving_jaws = 0;
    for action in actions {
        let core = cores
            .get(action.arm.as_str())
            .ok_or_else(|| anyhow::anyhow!("No arm {}", action.arm))?;
        let geometry = core.group_collision.as_ref().ok_or_else(|| {
            anyhow::anyhow!(
                "Arm {} needs a base pose and collision geometry for grouped motion",
                action.arm
            )
        })?;
        anyhow::ensure!(
            !core.gripper_command_pending(),
            "Wait for the gripper on {} before starting grouped motion",
            action.arm
        );
        let (initial, target) = match &action.movement {
            Some(AdmittedMovement::Trajectory { decision, .. }) => {
                moving_arms += 1;
                let corners = &decision.plan.segments[0].corners;
                (corners[0].as_slice(), corners[1].as_slice())
            }
            Some(AdmittedMovement::PolicyPose { .. }) => {
                anyhow::bail!(
                    "Grouped clearance cannot check policy motion on {}",
                    action.arm
                );
            }
            None => {
                let joints = core.stationary_joints().ok_or_else(|| {
                    anyhow::anyhow!(
                        "Arm {} must be holding with joint feedback for grouped gripper motion",
                        action.arm
                    )
                })?;
                (joints, joints)
            }
        };
        anyhow::ensure!(
            !geometry.has_gripper(initial.len()) || geometry.gripper_angle.is_some(),
            "Arm {} needs jaw feedback for grouped motion",
            action.arm
        );
        let target_jaw = action
            .gripper
            .map(|target| target.angle.radians())
            .or(geometry.gripper_angle);
        if target_jaw != geometry.gripper_angle {
            moving_jaws += 1;
        }
        let initial = geometry.joints_with_gripper(initial, geometry.gripper_angle);
        let target = geometry.joints_with_gripper(target, target_jaw);
        members.push((&action.arm, geometry, initial, target));
    }
    // SAFETY: A jaw advances independently from the grouped joint trajectory and
    // other jaws. Only sweep one independent motion at a time.
    anyhow::ensure!(
        moving_jaws == 0 || (moving_jaws == 1 && moving_arms == 0),
        "Use separate steps for grouped arm motion and each jaw movement"
    );
    let start = members
        .iter()
        .flat_map(|(_, _, start, _)| start.iter().copied())
        .collect::<Vec<_>>();
    let end = members
        .iter()
        .flat_map(|(_, _, _, end)| end.iter().copied())
        .collect::<Vec<_>>();
    let mut joints = members
        .iter()
        .map(|(_, _, start, _)| start.clone())
        .collect::<Vec<_>>();
    let mut workspace = CollisionWorkspace::default();
    for fraction in std::iter::once(0.0).chain(crate::robot::path_sample_fractions(&start, &end)) {
        for ((_, _, start, end), arm_joints) in members.iter().zip(&mut joints) {
            crate::robot::interpolate_path_sample(start, end, fraction, arm_joints);
        }
        let bodies = members
            .iter()
            .zip(&joints)
            .map(|((_, geometry, _, _), joints)| geometry.body(joints))
            .collect::<Vec<_>>();
        if let Some(hit) = workspace.scene_collision(&bodies, INTER_ARM_PENETRATION_METERS)? {
            let [first, second] = hit.bodies;
            let [first_link, second_link] = hit.links;
            anyhow::bail!(
                "Grouped motion would exceed allowed penetration between {}:{first_link} and {}:{second_link}",
                members[first].0,
                members[second].0
            );
        }
    }
    Ok(())
}

// INVARIANT: Validate every resource before admitting the entire step in one control-loop cycle.
#[allow(
    clippy::too_many_lines,
    reason = "Keep validation, admission and failure cleanup together at the atomic boundary."
)]
#[expect(
    clippy::expect_used,
    reason = "Every arm is validated before admission and the exclusively borrowed map does not change membership."
)]
pub(crate) fn admit_action_step(
    cores: &mut BTreeMap<&str, &mut ArmCore>,
    actions: Vec<PreparedArmAction>,
    now: Now,
) -> Result<Vec<ArmRecord>, Rejection> {
    let invalid = |detail: String| Rejection {
        reason: RejectionReason::InvalidTarget,
        detail,
    };
    if actions.is_empty() {
        return Err(invalid("An action step needs at least one arm".into()));
    }
    let mut names = actions
        .iter()
        .map(PreparedArmAction::arm)
        .collect::<Vec<_>>();
    names.sort_unstable();
    if let Some([duplicate, _]) = names.windows(2).find(|pair| pair[0] == pair[1]) {
        return Err(invalid(format!("Arm {duplicate} appears twice")));
    }
    let scope = if actions.len() == 1 {
        MotionScope::Individual
    } else {
        MotionScope::Grouped
    };
    let trajectory_arms = actions
        .iter()
        .filter(|action| action.uses_trajectory())
        .map(|action| action.arm().to_owned())
        .collect::<Vec<_>>();
    let actions = actions
        .into_iter()
        .map(|action| {
            let core = cores
                .get(action.arm())
                .ok_or_else(|| invalid(format!("No arm {}", action.arm())))?;
            let arm = action.arm().to_owned();
            prepare_arm_action(core, action, scope).map_err(|refused| Rejection {
                reason: refused.reason,
                detail: format!("{arm}: {}", refused.detail),
            })
        })
        .collect::<Result<Vec<_>, Rejection>>()?;
    check_group_clearance(cores, &actions).map_err(|error| invalid(format!("{error:#}")))?;
    let group = GroupMembership {
        id: crate::arm::new_identifier().into(),
        arms: trajectory_arms,
    };
    let mut records = Vec::new();
    for action in actions {
        let core = cores
            .get_mut(action.arm.as_str())
            .expect("all arms validated");
        if let Some(movement) = action.movement {
            let record = match movement {
                AdmittedMovement::Trajectory {
                    label,
                    decision,
                    short,
                } => core.apply_move(
                    &label,
                    decision,
                    short,
                    matches!(scope, MotionScope::Grouped).then(|| group.clone()),
                    now,
                ),
                AdmittedMovement::PolicyPose { label, goal } => core.apply_reach(&label, goal, now),
            };
            records.push(ArmRecord {
                arm: action.arm.clone(),
                record,
            });
        }
        if let Some(target) = action.gripper {
            records.push(ArmRecord {
                arm: action.arm,
                record: core.apply_gripper(target, None, now),
            });
        }
    }
    Ok(records)
}

#[cfg(test)]
pub(crate) fn apply_single_request(request: ArmRequest, arm: &mut ArmCore) {
    let ArmRequest::Single(request) = request else {
        panic!("Expected a single-arm request");
    };
    request.apply(arm);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;
    use std::time::{Duration, Instant};

    use crate::arm::Limits;
    use crate::lease::{AcquireMode, GenerationTicket, Lease, LeaseAdmission, SharedInterlocks};
    use hebbian_contracts::{
        CommandOutcome, CornerPlan, LeaseHolder, LeaseRequest, OnInterruption, Segment,
    };

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
            joint_speed: hebbian_contracts::JointSpeed::new(1.0).unwrap(),
            path_limited: None,
            pose_limited: None,
        }
    }

    #[test]
    fn an_expired_manual_lease_allows_enable_only_after_every_arm_has_stopped() {
        let lease = Lease::new(Duration::from_secs(1));
        let request = LeaseRequest {
            holder: LeaseHolder::Manual,
            label: "viewer".into(),
            required_cameras: vec![],
            on_interruption: OnInterruption::WaitForApproval,
        };
        let mode = AcquireMode::WaitOrFail;
        let grant = lease.acquire_stopping_arms(request.clone(), mode).unwrap();
        let expired = Instant::now() + Duration::from_secs(2);
        let (disabled_handle, disabled_inbox) = arm_channel();
        let (moving_handle, moving_inbox) = arm_channel();
        let moving_generation = Arc::new(AtomicU64::new(0));
        lease.register_arm(
            disabled_handle,
            Arc::new(AtomicU64::new(0)),
            SharedInterlocks::default(),
        );
        lease.register_arm(
            moving_handle,
            Arc::clone(&moving_generation),
            SharedInterlocks::default(),
        );
        let prepared_before_expiry = [GenerationTicket::capture(&moving_generation)];
        let mut disabled = core();
        let mut moving = holding_core();
        let movement = part("moving", [0.5, 0.0]);
        moving
            .submit(
                Command::Move {
                    label: "move_to_joints".into(),
                    plan: movement.plan,
                    joint_speed: movement.joint_speed,
                    path_limited: None,
                    pose_limited: None,
                },
                Now::current(),
            )
            .unwrap();
        moving.next_job().unwrap();
        moving.begin_moving();
        let mut cores = BTreeMap::from([("disabled", &mut disabled), ("moving", &mut moving)]);

        assert!(lease.status(expired).is_some());
        assert!(
            moving_inbox.next().is_none(),
            "reading status may not stop motion"
        );
        lease.tick(expired);
        assert!(lease.status(expired).is_some());
        assert!(
            lease
                .admit_generation(None, expired, &[], &[], LeaseAdmission::Normal, || ())
                .is_err()
        );
        for (name, inbox) in [("disabled", &disabled_inbox), ("moving", &moving_inbox)] {
            while let Some(request) = inbox.next() {
                apply_single_request(request, cores.get_mut(name).unwrap());
            }
        }
        // WHY: The stop was accepted, but the active movement has not ended yet.
        lease.tick(expired);
        assert!(lease.status(expired).is_some());
        cores.get_mut("moving").unwrap().motion_ended(
            CommandOutcome::Stopped,
            None,
            &[0.2, 0.0],
            Now::current(),
        );
        while let Some(request) = moving_inbox.next() {
            apply_single_request(request, cores.get_mut("moving").unwrap());
        }
        lease.tick(expired);
        assert!(lease.status(expired).is_none());
        assert!(!lease.retain(Some(&grant.lease_token), expired));
        let enabled = lease
            .admit_generation(None, expired, &[], &[], LeaseAdmission::Normal, || {
                cores
                    .get_mut("disabled")
                    .unwrap()
                    .submit(Command::Enable, Now::current())
            })
            .unwrap()
            .unwrap();
        assert_eq!(enabled.label, "enable");
        let fresh = lease.acquire_stopping_arms(request, mode).unwrap();
        assert_ne!(fresh.lease_token, grant.lease_token);
        let late_plan = lease.admit_generation(
            Some(&grant.lease_token),
            expired,
            &[],
            &prepared_before_expiry,
            LeaseAdmission::Normal,
            || (),
        );
        assert!(matches!(late_plan, Err(error) if error.reason == RejectionReason::ArmBusy));
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
        apply_single_request(request, &mut owned);
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
        apply_single_request(inbox.next().unwrap(), &mut owned);
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
        while let Some(request) = inbox.next() {
            apply_single_request(request, &mut owned);
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
    fn a_group_is_refused_when_either_arm_lacks_collision_geometry() {
        let mut left = holding_core();
        let mut right = holding_core();
        let mut cores = BTreeMap::from([("left", &mut left), ("right", &mut right)]);
        let refusal = admit_group(
            &mut cores,
            "together",
            vec![part("left", [0.5, 0.0]), part("right", [0.0, 0.5])],
            Now::current(),
        )
        .unwrap_err();
        assert!(
            refusal
                .detail
                .contains("needs a base pose and collision geometry")
        );
        drop(cores);
        assert!(left.next_job().is_none() && right.next_job().is_none());
    }

    fn so101_collision_guard() -> std::sync::Arc<crate::self_collision::SelfCollisionGuard> {
        use crate::robot::Robot;
        use crate::self_collision::SelfCollisionGuard;
        use std::sync::Arc;

        let names = [
            "shoulder_pan",
            "shoulder_lift",
            "elbow_flex",
            "wrist_flex",
            "wrist_roll",
            "gripper",
        ]
        .map(str::to_owned);
        let urdf = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../robots/so101/robot-soft-gripper-camera.urdf");
        let robot = Arc::new(Robot::load(&urdf, &names).unwrap());
        Arc::new(SelfCollisionGuard::new(robot, 0.01, &[]).unwrap())
    }

    fn so101_joints(pan: f64) -> Vec<f64> {
        vec![pan, -1.0, 1.0, 0.5, 0.0]
    }

    fn so101_group_arm(
        guard: std::sync::Arc<crate::self_collision::SelfCollisionGuard>,
        direction: f64,
        start: f64,
        jaw: f64,
    ) -> ArmCore {
        let mut arm = ArmCore::new(
            Limits {
                joint_count: 5,
                lower: vec![-1.5; 5],
                upper: vec![1.5; 5],
                joint_limit_recovery_tolerance_radians: 0.035,
                gripper: Some((0.0, 1.5)),
                pose_goals: Some(crate::arm::PoseGoalSettings {
                    position_tolerance_meters: 0.01,
                    orientation_tolerance_radians: 0.1,
                }),
                jaw_closing_axis: hebbian_contracts::JawClosingAxis::X,
                default_joint_speed: 1.0,
                max_joint_speed: 2.0,
            },
            |_| {},
        );
        arm.group_collision = Some(crate::self_collision::GroupCollisionArm {
            guard,
            base: nalgebra::Isometry3::translation(0.0, direction * 0.190_574_745, 0.0),
            gripper_angle: Some(jaw),
        });
        let now = Now::current();
        let joints = so101_joints(direction * start);
        arm.observe(now, &joints, None, None);
        arm.enabled(&joints, now);
        arm
    }

    fn so101_group_move(name: &str, direction: f64, start: f64, target: f64) -> ArmMove {
        let mut movement = part(name, [0.0, 0.0]);
        movement.plan.segments[0].corners = vec![
            so101_joints(direction * start),
            so101_joints(direction * target),
        ];
        movement
    }

    #[test]
    fn group_admission_allows_a_close_pass_and_light_contact_but_refuses_a_crossing() {
        let guard = so101_collision_guard();
        for (start, target, jaw, accepted) in [
            (0.5, 0.8, 0.0, true),
            (0.8, 0.92, 0.0, true),
            (0.8, 1.05, 0.0, false),
            (0.8, 1.05, 1.5, false),
        ] {
            let mut left = so101_group_arm(guard.clone(), 1.0, start, jaw);
            let mut right = so101_group_arm(guard.clone(), -1.0, start, jaw);
            let mut cores = BTreeMap::from([("left", &mut left), ("right", &mut right)]);
            let outcome = admit_group(
                &mut cores,
                "crossing",
                vec![
                    so101_group_move("left", 1.0, start, target),
                    so101_group_move("right", -1.0, start, target),
                ],
                Now::current(),
            );
            if accepted {
                let records = outcome.unwrap();
                assert_eq!(records.len(), 2);
                assert_eq!(records[0].group_id(), records[1].group_id());
                assert!(records[0].group_id().is_some());
            } else {
                assert!(outcome.unwrap_err().detail.contains("allowed penetration"));
            }
            assert_eq!(left.next_job().is_some(), accepted);
            assert_eq!(right.next_job().is_some(), accepted);
        }
    }

    #[test]
    fn grouped_motion_refuses_independent_jaw_or_policy_motion_before_either_arm_starts() {
        let guard = so101_collision_guard();
        for (right_action, reason) in [
            (
                PreparedArmAction::movement(
                    "right".into(),
                    PreparedMovement::PolicyPose {
                        label: "move_to_pose".into(),
                        position: [0.2, 0.0, 0.2],
                        orientation: hebbian_contracts::PoseOrientation::Quaternion {
                            quaternion: [0.0, 0.0, 0.0, 1.0],
                        },
                    },
                ),
                "cannot check policy motion",
            ),
            (
                PreparedArmAction::gripper(
                    "right".into(),
                    crate::action_step::PreparedGripper {
                        angle: hebbian_contracts::JawAngle::try_from(0.2).unwrap(),
                        joint_speed: None,
                    },
                ),
                "separate steps",
            ),
        ] {
            let mut left = so101_group_arm(guard.clone(), 1.0, 0.5, 0.0);
            let mut right = so101_group_arm(guard.clone(), -1.0, 0.5, 0.0);
            let mut cores = BTreeMap::from([("left", &mut left), ("right", &mut right)]);
            let movement = so101_group_move("left", 1.0, 0.5, 0.8);
            let left_action = PreparedArmAction::from_group_move("move_to_joints", movement);
            let error =
                admit_action_step(&mut cores, vec![left_action, right_action], Now::current())
                    .unwrap_err();
            assert!(error.detail.contains(reason), "{error:?}");
            assert!(left.next_job().is_none());
            assert!(right.next_job().is_none());
            assert!(!right.gripper_command_pending());
        }
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
        assert!(detail(vec![]).contains("at least one arm"));
        assert_eq!(
            detail(vec![part("only", [0.1, 0.0]), part("only", [0.1, 0.0])]),
            "Arm only appears twice"
        );
        assert_eq!(detail(vec![part("gone", [0.1, 0.0])]), "No arm gone");
    }

    proptest::proptest! {
        #![proptest_config(proptest::test_runner::Config::with_cases(96))]

        #[test]
        fn a_rejected_group_leaves_every_arm_and_its_existing_records_unchanged(
            target in -1.9_f64..1.9, outside in 2.001_f64..20.0
        ) {
            use proptest::{prop_assert, prop_assert_eq};
            let mut left = holding_core();
            let mut right = holding_core();
            let left_poses = left.poses().cloned();
            let right_poses = right.poses().cloned();
            let mut cores = BTreeMap::from([("left", &mut left), ("right", &mut right)]);
            prop_assert!(admit_group(&mut cores, "generated", vec![part("left", [target, 0.0]), part("right", [outside, 0.0])], Now::current()).is_err());
            drop(cores);
            prop_assert_eq!(left.poses(), left_poses.as_ref());
            prop_assert_eq!(right.poses(), right_poses.as_ref());
            prop_assert!(left.next_job().is_none());
            prop_assert!(right.next_job().is_none());
            prop_assert!(left.last_finished().is_none());
            prop_assert!(right.last_finished().is_none());
        }
    }
}

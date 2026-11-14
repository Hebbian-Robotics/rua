//! Tests of the arm state machine.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use super::admission::RETAINED_RECORDS;
use super::*;
use hebbian_contracts::{
    ArmMode, Command, CommandOutcome, CommandRecord, CornerPlan, Event, JawClosingAxis,
    RejectionReason, Segment, TargetStatus,
};

fn limits() -> Limits {
    Limits {
        joint_count: 2,
        lower: vec![-2.0, -2.0],
        upper: vec![2.0, 2.0],
        joint_limit_recovery_tolerance_radians: 0.035,
        gripper: Some((0.0, 1.0)),
        pose_goals: Some(PoseGoalSettings {
            position_tolerance_meters: 0.02,
            orientation_tolerance_radians: 0.1,
        }),
        jaw_closing_axis: JawClosingAxis::X,
        default_joint_speed: 1.0,
        max_joint_speed: 2.0,
    }
}

fn core() -> (ArmCore, Arc<Mutex<Vec<Event>>>) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&events);
    let core = ArmCore::new(limits(), move |event| sink.lock().unwrap().push(event));
    (core, events)
}

fn at(seconds: f64) -> Now {
    Now {
        unix_seconds: seconds,
        instant: Instant::now(),
    }
}

fn plan(from: [f64; 2], to: [f64; 2]) -> CornerPlan {
    CornerPlan {
        segments: vec![Segment {
            label: "go".into(),
            corners: vec![from.to_vec(), to.to_vec()],
            minimum_duration_seconds: None,
            target_position: None,
            target_quaternion: None,
        }],
        peak_speed: 1.0,
    }
}

fn mv(from: [f64; 2], to: [f64; 2]) -> Command {
    mv_planned(from, to)
}

/// A move from `from` to `to`.
fn mv_planned(from: [f64; 2], to: [f64; 2]) -> Command {
    Command::Move {
        label: "move_to_joints".into(),
        plan: plan(from, to),
        joint_speed: 1.0,
        path_limited: None,
        pose_limited: None,
    }
}

fn observe(core: &mut ArmCore, seconds: f64, fault: Option<&str>) {
    core.observe(seconds, &[0.0, 0.0], None, fault);
}

/// Enable through the same steps the loop takes.
fn enabled_core() -> (ArmCore, Arc<Mutex<Vec<Event>>>) {
    let (mut core, events) = core();
    let record = core.submit(Command::Enable, at(1.0)).unwrap();
    let job = core.next_job().unwrap();
    assert_eq!(job.command_id(), record.command_id);
    observe(&mut core, 1.0, None);
    core.enabled(&[0.0, 0.0], at(1.0));
    (core, events)
}

fn reason(result: Result<CommandRecord, Rejection>) -> RejectionReason {
    result.unwrap_err().reason
}

#[test]
fn a_disabled_arm_accepts_only_enable() {
    let (mut core, _) = core();
    assert_eq!(
        reason(core.submit(mv([0.0, 0.0], [0.1, 0.0]), at(0.0))),
        RejectionReason::NotEnabled
    );
    assert_eq!(
        reason(core.submit(Command::Disable, at(0.0))),
        RejectionReason::NotEnabled
    );
    assert!(core.submit(Command::Enable, at(0.0)).is_ok());
    // Enabling twice is busy, not queued twice.
    assert_eq!(
        reason(core.submit(Command::Enable, at(0.0))),
        RejectionReason::ArmBusy
    );
}

#[test]
fn enabling_holds_the_present_pose_and_completes_its_record() {
    let (core, events) = enabled_core();
    assert_eq!(core.mode(), ArmMode::Holding);
    assert_eq!(core.poses().unwrap().hold, vec![0.0, 0.0]);
    let finished = core.last_finished().unwrap();
    assert_eq!(finished.label, "enable");
    assert_eq!(finished.outcome(), Some(CommandOutcome::Completed));
    assert!(events.lock().unwrap().iter().any(|event| matches!(
        event,
        Event::Mode {
            mode: ArmMode::Holding,
            ..
        }
    )));
}

#[test]
fn a_move_is_queued_with_its_target_and_refused_while_another_is_active() {
    let (mut core, _) = enabled_core();
    let record = core.submit(mv([0.0, 0.0], [0.5, -0.5]), at(2.0)).unwrap();
    assert_eq!(record.target_joints, Some(vec![0.5, -0.5]));
    assert_eq!(record.joint_speed, Some(1.0));
    // Queued commands occupy the channel just as running ones do.
    assert_eq!(
        reason(core.submit(mv([0.0, 0.0], [0.1, 0.0]), at(2.0))),
        RejectionReason::ArmBusy
    );
    let job = core.next_job().unwrap();
    assert!(matches!(job, Job::Move { .. }));
    core.begin_moving();
    assert_eq!(core.mode(), ArmMode::Moving);
    assert_eq!(core.active_command_id(), Some(record.command_id.as_str()));
}

#[test]
fn a_plan_from_another_pose_starts_from_where_the_arm_holds() {
    let (mut core, _) = enabled_core();
    core.submit(mv([0.3, 1e-7], [0.5, 0.0]), at(2.0)).unwrap();
    let Some(Job::Move { plan, .. }) = core.next_job() else {
        panic!("no move queued");
    };
    assert_eq!(
        plan.segments[0].corners,
        vec![vec![0.0, 0.0], vec![0.5, 0.0]]
    );
}

#[test]
fn a_gripper_goal_past_its_range_is_clamped_and_a_non_finite_one_refused() {
    let (mut core, _) = enabled_core();
    for (asked, sent) in [(1.5, 1.0), (-0.2, 0.0), (1.0, 1.0)] {
        core.submit(
            Command::SetGripper {
                angle: asked,
                joint_speed: None,
            },
            at(2.0),
        )
        .unwrap();
        assert_eq!(
            core.take_gripper_target()
                .map(|(_, target)| target.angle_radians),
            Some(sent)
        );
    }
    assert_eq!(
        reason(core.submit(
            Command::SetGripper {
                angle: f64::NAN,
                joint_speed: None
            },
            at(2.0)
        )),
        RejectionReason::InvalidTarget
    );
    for (requested, expected) in [(None, 1.0), (Some(0.4), 0.4), (Some(9.0), 2.0)] {
        let record = core
            .submit(
                Command::SetGripper {
                    angle: 0.5,
                    joint_speed: requested,
                },
                at(2.0),
            )
            .unwrap();
        assert_eq!(record.joint_speed, Some(expected));
        assert_eq!(
            core.take_gripper_target()
                .unwrap()
                .1
                .max_speed_radians_per_second,
            expected
        );
    }
    assert_eq!(
        reason(core.submit(
            Command::SetGripper {
                angle: 0.5,
                joint_speed: Some(0.0)
            },
            at(2.0)
        )),
        RejectionReason::InvalidTarget
    );
}

#[test]
fn a_completed_move_records_how_close_the_arm_ended_to_its_target() {
    let (mut core, _) = enabled_core();
    let record = core.submit(mv([0.0, 0.0], [0.5, 0.0]), at(2.0)).unwrap();
    core.next_job().unwrap();
    core.begin_moving();
    core.observe(3.0, &[0.49, 0.0], None, None);
    core.motion_ended(CommandOutcome::Completed, None, &[0.5, 0.0], at(3.0));
    let finished = core.record(&record.command_id).unwrap();
    assert_eq!(finished.target_status, Some(TargetStatus::Reached));
    let achieved = finished.achieved.as_ref().unwrap();
    assert!((achieved.joint_error_radians.unwrap() - 0.01).abs() < 1e-12);
    assert_eq!(core.mode(), ArmMode::Holding);
    assert_eq!(core.poses().unwrap().hold, vec![0.5, 0.0]);
    assert_eq!(core.active_command_id(), None);
}

#[test]
fn stop_ends_the_queue_asks_the_active_command_to_stop_and_completes_itself() {
    let (mut core, _) = enabled_core();
    let record = core.submit(mv([0.0, 0.0], [0.5, 0.0]), at(2.0)).unwrap();
    // Stopped before it started: a queued command ends stopped, not started.
    let stop = core.submit(Command::Stop, at(2.5)).unwrap();
    assert_eq!(stop.label, "stop");
    let cancelled = core.record(&record.command_id).unwrap();
    assert_eq!(cancelled.outcome(), Some(CommandOutcome::Stopped));
    assert_eq!(cancelled.target_status, Some(TargetStatus::NotStarted));
    assert!(core.next_job().is_none());
    // Stopped while running: the loop sees the request and ends it partial.
    let running = core
        .submit(mv_planned([0.0, 0.0], [0.5, 0.0]), at(3.0))
        .unwrap();
    core.next_job().unwrap();
    core.begin_moving();
    core.submit(Command::Stop, at(3.5)).unwrap();
    assert!(core.stop_requested());
    core.motion_ended(
        CommandOutcome::Stopped,
        Some("Stopped".into()),
        &[0.2, 0.0],
        at(3.6),
    );
    let ended = core.record(&running.command_id).unwrap();
    assert_eq!(ended.outcome(), Some(CommandOutcome::Stopped));
    assert_eq!(ended.target_status, Some(TargetStatus::Partial));
    assert!(!core.stop_requested());
}

#[test]
fn control_handoff_waits_for_the_running_motion_to_stop() {
    let (mut core, _) = enabled_core();
    core.submit(mv([0.0, 0.0], [0.5, 0.0]), at(2.0)).unwrap();
    core.next_job().unwrap();
    core.begin_moving();
    let stop = core.submit(Command::Stop, at(2.5)).unwrap();
    assert_eq!(stop.outcome(), Some(CommandOutcome::Completed));
    assert!(!core.ready_for_control_handoff().unwrap());

    core.motion_ended(CommandOutcome::Stopped, None, &[0.2, 0.0], at(2.6));
    assert!(!core.ready_for_control_handoff().unwrap());
    assert!(core.take_gripper_stop());
    assert!(core.ready_for_control_handoff().unwrap());
}

#[test]
fn a_stream_takes_goals_by_id_until_it_ends_and_then_says_so() {
    let (mut core, _) = enabled_core();
    let start = core
        .submit(
            Command::StreamJoints {
                gripper_angle: None,
                positions: vec![0.1, 3.0],
                joint_speed: None,
                stream_command_id: None,
            },
            at(2.0),
        )
        .unwrap();
    let wrong_length = core.submit(
        Command::StreamJoints {
            gripper_angle: None,
            positions: vec![1.0],
            joint_speed: None,
            stream_command_id: None,
        },
        at(2.0),
    );
    assert_eq!(reason(wrong_length), RejectionReason::InvalidTarget);
    // Goals past the limits are clamped, and the record shows what is in use.
    assert_eq!(start.goal_joints, Some(vec![0.1, 2.0]));
    assert_eq!(start.joint_speed, Some(1.0));
    let update = core
        .submit(
            Command::StreamJoints {
                gripper_angle: None,
                positions: vec![0.2, 0.0],
                joint_speed: Some(9.0),
                stream_command_id: Some(start.command_id.clone()),
            },
            at(2.1),
        )
        .unwrap();
    assert_eq!(update.command_id, start.command_id);
    assert_eq!(update.goal_joints, Some(vec![0.2, 0.0]));
    core.next_job().unwrap();
    core.begin_moving();
    core.motion_ended(
        CommandOutcome::Stopped,
        Some("No goal for 0.5 s".into()),
        &[0.2, 0.0],
        at(3.0),
    );
    let late = core.submit(
        Command::StreamJoints {
            gripper_angle: None,
            positions: vec![0.3, 0.0],
            joint_speed: None,
            stream_command_id: Some(start.command_id),
        },
        at(3.1),
    );
    let refused = late.unwrap_err();
    assert_eq!(refused.reason, RejectionReason::StreamEnded);
    assert!(refused.detail.contains("stopped"), "{}", refused.detail);
}

#[test]
fn a_disconnected_motion_has_an_unknown_outcome() {
    let (mut core, _) = enabled_core();
    let record = core.submit(mv([0.0, 0.0], [0.5, 0.0]), at(2.0)).unwrap();
    core.next_job().unwrap();
    core.begin_moving();
    core.bus_lost("Bus stopped answering", &[], at(3.0));
    assert_eq!(core.mode(), ArmMode::Disconnected);
    assert_eq!(
        core.record(&record.command_id).unwrap().outcome(),
        Some(CommandOutcome::Interrupted)
    );
    let interrupted = core.record(&record.command_id).unwrap();
    assert_eq!(interrupted.achieved, None);
    assert_eq!(interrupted.target_status, None);
}

#[test]
fn a_hard_fault_releases_and_refuses_everything_until_cleared_on_fresh_healthy_feedback() {
    let (mut core, _) = enabled_core();
    let record = core.submit(mv([0.0, 0.0], [0.5, 0.0]), at(2.0)).unwrap();
    core.next_job().unwrap();
    core.begin_moving();
    core.enter_fault("Feedback unavailable", &[], at(3.0));
    assert_eq!(core.mode(), ArmMode::Faulted);
    assert_eq!(
        core.record(&record.command_id).unwrap().outcome(),
        Some(CommandOutcome::Faulted)
    );
    assert_eq!(
        reason(core.submit(Command::Enable, at(3.1))),
        RejectionReason::Faulted
    );
    assert_eq!(
        reason(core.submit(mv([0.0, 0.0], [0.1, 0.0]), at(3.1))),
        RejectionReason::Faulted
    );
    // No feedback since the fault.
    assert_eq!(
        reason(core.submit(Command::ClearFault, at(3.5))),
        RejectionReason::Faulted
    );
    // Feedback after the fault, but it reports a problem.
    observe(&mut core, 3.4, Some("1: 80C"));
    assert!(
        core.submit(Command::ClearFault, at(3.5))
            .unwrap_err()
            .detail
            .contains("80C")
    );
    // Stale feedback is not recent.
    observe(&mut core, 3.4, None);
    assert_eq!(
        reason(core.submit(Command::ClearFault, at(9.0))),
        RejectionReason::Faulted
    );
    observe(&mut core, 3.4, None);
    assert!(core.submit(Command::ClearFault, at(3.5)).is_ok());
    assert_eq!(core.mode(), ArmMode::Disabled);
    assert_eq!(core.fault(), None);
}

#[test]
fn releasing_ends_the_jaw_command_and_forgets_the_poses() {
    let (mut core, _) = enabled_core();
    let gripper = core
        .submit(
            Command::SetGripper {
                angle: 0.5,
                joint_speed: None,
            },
            at(2.0),
        )
        .unwrap();
    let release = core.submit(Command::Disable, at(2.0)).unwrap();
    core.next_job().unwrap();
    core.released(&[], at(2.1));
    assert_eq!(core.mode(), ArmMode::Disabled);
    assert_eq!(core.poses(), None);
    assert_eq!(
        core.record(&gripper.command_id).unwrap().outcome(),
        Some(CommandOutcome::Stopped)
    );
    assert_eq!(
        core.record(&release.command_id).unwrap().outcome(),
        Some(CommandOutcome::Completed)
    );
}

#[test]
fn a_newer_gripper_target_replaces_the_unfinished_one() {
    let (mut core, _) = enabled_core();
    let first = core
        .submit(
            Command::SetGripper {
                angle: 0.2,
                joint_speed: None,
            },
            at(2.0),
        )
        .unwrap();
    let second = core
        .submit(
            Command::SetGripper {
                angle: 0.8,
                joint_speed: None,
            },
            at(2.1),
        )
        .unwrap();
    assert_eq!(
        core.record(&first.command_id).unwrap().outcome(),
        Some(CommandOutcome::Stopped)
    );
    let (command_id, target) = core.take_gripper_target().unwrap();
    assert_eq!(command_id, second.command_id);
    assert_eq!(target.angle_radians, 0.8);
    assert_eq!(target.max_speed_radians_per_second, 1.0);
}

#[test]
fn repeating_shutdown_does_not_interrupt_the_return_to_rest() {
    let (mut core, _) = enabled_core();
    core.submit(Command::Shutdown, at(1.0)).unwrap();
    core.allow_shutdown_return();
    assert!(!core.stop_requested());
    core.submit(Command::Shutdown, at(2.0)).unwrap();
    assert!(!core.stop_requested());
}

#[test]
fn shutdown_refuses_new_commands_and_stops_what_is_running() {
    let (mut core, _) = enabled_core();
    let record = core.submit(mv([0.0, 0.0], [0.5, 0.0]), at(2.0)).unwrap();
    core.next_job().unwrap();
    core.begin_moving();
    core.submit(Command::Shutdown, at(2.5)).unwrap();
    assert!(core.shutdown_requested());
    assert!(core.stop_requested());
    assert_eq!(
        reason(core.submit(mv([0.0, 0.0], [0.1, 0.0]), at(2.6))),
        RejectionReason::ArmBusy
    );
    core.motion_ended(CommandOutcome::Stopped, None, &[0.1, 0.0], at(2.7));
    assert_eq!(
        core.record(&record.command_id).unwrap().outcome(),
        Some(CommandOutcome::Stopped)
    );
    // The return to rest is allowed to run to its end.
    core.allow_shutdown_return();
    assert!(!core.stop_requested());
}

#[test]
fn retained_records_are_bounded_and_the_newest_survive() {
    let (mut core, _) = enabled_core();
    for index in 0..(RETAINED_RECORDS + 20) {
        let record = core
            .submit(
                Command::Stop,
                at(2.0 + f64::from(u16::try_from(index).unwrap())),
            )
            .unwrap();
        assert!(core.record(&record.command_id).is_some());
    }
    assert!(core.records.len() <= RETAINED_RECORDS);
}
#[test]
fn a_combined_stream_is_rejected_before_either_target_changes() {
    let (mut core, _) = enabled_core();
    let refused = core.submit(
        Command::StreamJoints {
            positions: vec![0.1, 0.0],
            gripper_angle: Some(f64::NAN),
            joint_speed: None,
            stream_command_id: None,
        },
        at(2.0),
    );
    assert_eq!(refused.unwrap_err().reason, RejectionReason::InvalidTarget);
    assert!(core.next_job().is_none());
    assert!(core.take_gripper_target().is_none());
    let refused = core.submit(
        Command::StreamJoints {
            positions: vec![0.1],
            gripper_angle: Some(0.5),
            joint_speed: None,
            stream_command_id: None,
        },
        at(2.5),
    );
    assert_eq!(refused.unwrap_err().reason, RejectionReason::InvalidTarget);
    assert!(core.next_job().is_none());
    assert!(core.take_gripper_target().is_none());
    let accepted = core
        .submit(
            Command::StreamJoints {
                positions: vec![0.1, 0.0],
                gripper_angle: Some(0.5),
                joint_speed: None,
                stream_command_id: None,
            },
            at(3.0),
        )
        .unwrap();
    assert_eq!(accepted.goal_joints, Some(vec![0.1, 0.0]));
    assert!(core.next_job().is_some());
    assert_eq!(core.take_gripper_target().unwrap().1.angle_radians, 0.5);
}

#[test]
fn a_move_replaces_a_settling_one_from_the_measured_joints_and_reports_it_superseded() {
    let (mut core, _) = enabled_core();
    let settling = core.submit(mv([0.0, 0.0], [0.5, 0.0]), at(2.0)).unwrap();
    core.next_job().unwrap();
    core.begin_moving();
    // While the path still runs, another command is refused.
    assert_eq!(
        reason(core.submit(mv([0.0, 0.0], [0.1, 0.0]), at(2.5))),
        RejectionReason::ArmBusy
    );
    core.observe(3.0, &[0.45, 0.0], None, None);
    core.set_settling(true);
    let replacement = core.submit(mv([0.5, 0.0], [0.0, 0.3]), at(3.0)).unwrap();
    let superseded = core.record(&settling.command_id).unwrap();
    assert_eq!(superseded.outcome(), Some(CommandOutcome::Superseded));
    let errors = superseded.achieved.as_ref().unwrap();
    let joint_errors = errors.joint_errors_radians.clone().unwrap();
    assert!((joint_errors[0] - 0.05).abs() < 1e-12 && joint_errors[1] == 0.0);
    assert!(core.take_supersede());
    core.superseded_motion_ended(&[0.45, 0.0]);
    let Some(Job::Move { plan, .. }) = core.next_job() else {
        panic!("the replacement is queued");
    };
    assert_eq!(plan.segments[0].corners[0], vec![0.45, 0.0]);
    assert_eq!(
        core.active_command_id(),
        Some(replacement.command_id.as_str())
    );
}

#[test]
fn stopping_a_replacement_cancels_it_and_releases_the_superseded_motion() {
    let (mut core, _) = enabled_core();
    let first = core.submit(mv([0.0, 0.0], [0.5, 0.0]), at(2.0)).unwrap();
    core.next_job().unwrap();
    core.begin_moving();
    core.observe(3.0, &[0.45, 0.0], None, None);
    core.set_settling(true);
    let replacement = core.submit(mv([0.5, 0.0], [0.0, 0.3]), at(3.0)).unwrap();
    core.submit(Command::Stop, at(3.1)).unwrap();
    assert_eq!(
        core.record(&first.command_id).unwrap().outcome(),
        Some(CommandOutcome::Superseded)
    );
    assert_eq!(
        core.record(&replacement.command_id).unwrap().outcome(),
        Some(CommandOutcome::Stopped)
    );
    assert!(core.take_supersede());
    core.superseded_motion_ended(&[0.45, 0.0]);
    assert!(core.next_job().is_none());
    assert_eq!(core.mode(), ArmMode::Holding);
}

#[test]
fn a_fault_finishes_a_replacement_that_has_not_started() {
    let (mut core, _) = enabled_core();
    core.submit(mv([0.0, 0.0], [0.5, 0.0]), at(2.0)).unwrap();
    core.next_job().unwrap();
    core.begin_moving();
    core.observe(3.0, &[0.45, 0.0], None, None);
    core.set_settling(true);
    let replacement = core.submit(mv([0.5, 0.0], [0.0, 0.3]), at(3.0)).unwrap();
    core.enter_fault("lost torque", &[], at(3.1));
    assert_eq!(core.mode(), ArmMode::Faulted);
    assert_eq!(
        core.record(&replacement.command_id).unwrap().outcome(),
        Some(CommandOutcome::Stopped)
    );
    assert!(core.next_job().is_none());
}

#[test]
fn a_move_that_stopped_short_is_not_reported_reached() {
    let (mut core, _) = enabled_core();
    let record = core.submit(mv([0.0, 0.0], [0.5, 0.0]), at(2.0)).unwrap();
    core.next_job().unwrap();
    core.begin_moving();
    core.observe(3.0, &[0.449, 0.0], None, None);
    core.motion_ended(CommandOutcome::StoppedShort, None, &[0.5, 0.0], at(3.0));
    let finished = core.record(&record.command_id).unwrap();
    assert_eq!(finished.target_status, Some(TargetStatus::StoppedShort));
}

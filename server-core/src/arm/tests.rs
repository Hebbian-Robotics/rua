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
    let core = ArmCore::new(limits(), move |event| {
        sink.lock().unwrap().push(event.into());
    });
    (core, events)
}

static TEST_EPOCH: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

fn at(seconds: f64) -> Now {
    Now {
        unix_seconds: seconds,
        instant: *TEST_EPOCH.get_or_init(Instant::now)
            + std::time::Duration::from_secs_f64(seconds),
    }
}

fn plan(from: [f64; 2], to: [f64; 2]) -> CornerPlan {
    CornerPlan {
        segments: vec![Segment {
            pass_through: None,
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

fn mv_planned(from: [f64; 2], to: [f64; 2]) -> Command {
    Command::Move {
        label: "move_to_joints".into(),
        plan: plan(from, to),
        joint_speed: hebbian_contracts::JointSpeed::new(1.0).unwrap(),
        path_limited: None,
        pose_limited: None,
    }
}

fn observe(core: &mut ArmCore, seconds: f64, fault: Option<&str>) {
    core.observe(at(seconds), &[0.0, 0.0], None, fault);
}

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
    assert_eq!(record.target_joints(), Some([0.5, -0.5].as_slice()));
    assert_eq!(record_speed(&record), 1.0);
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
                angle: hebbian_contracts::JawAngle::try_from(asked).unwrap(),
                joint_speed: None,
            },
            at(2.0),
        )
        .unwrap();
        assert_eq!(
            core.take_gripper_target()
                .map(|(_, target)| target.angle.radians()),
            Some(sent)
        );
    }
    assert!(hebbian_contracts::JawAngle::try_from(f64::NAN).is_err());
    for (requested, expected) in [(None, 1.0), (Some(0.4), 0.4), (Some(9.0), 2.0)] {
        let record = core
            .submit(
                Command::SetGripper {
                    angle: hebbian_contracts::JawAngle::try_from(0.5).unwrap(),
                    joint_speed: requested
                        .map(|speed| hebbian_contracts::JointSpeed::new(speed).unwrap()),
                },
                at(2.0),
            )
            .unwrap();
        assert_eq!(record_speed(&record), expected);
        assert_eq!(
            core.take_gripper_target()
                .unwrap()
                .1
                .speed
                .radians_per_second(),
            expected
        );
    }
    assert!(
        serde_json::from_value::<Command>(serde_json::json!({
            "kind": "set_gripper", "angle": 0.5, "joint_speed": 0.0
        }))
        .is_err()
    );
}

#[test]
fn a_completed_move_records_how_close_the_arm_ended_to_its_target() {
    let (mut core, _) = enabled_core();
    let record = core.submit(mv([0.0, 0.0], [0.5, 0.0]), at(2.0)).unwrap();
    core.next_job().unwrap();
    core.begin_moving();
    core.observe(at(3.0), &[0.49, 0.0], None, None);
    core.motion_ended(CommandOutcome::Completed, None, &[0.5, 0.0], at(3.0));
    let finished = core.record(&record.command_id).unwrap();
    assert_eq!(finished.target_status(), Some(TargetStatus::Reached));
    let achieved = finished.achieved().unwrap();
    assert!((achieved.joint_error_radians.unwrap() - 0.01).abs() < 1e-12);
    assert_eq!(core.mode(), ArmMode::Holding);
    assert_eq!(core.poses().unwrap().hold, vec![0.5, 0.0]);
    assert_eq!(core.active_command_id(), None);
}

#[test]
fn stop_ends_the_queue_asks_the_active_command_to_stop_and_completes_itself() {
    let (mut core, _) = enabled_core();
    let record = core.submit(mv([0.0, 0.0], [0.5, 0.0]), at(2.0)).unwrap();
    let stop = core.submit(Command::Stop, at(2.5)).unwrap();
    assert_eq!(stop.label, "stop");
    let cancelled = core.record(&record.command_id).unwrap();
    assert_eq!(cancelled.outcome(), Some(CommandOutcome::Stopped));
    assert_eq!(cancelled.target_status(), Some(TargetStatus::NotStarted));
    assert!(core.next_job().is_none());
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
    assert_eq!(ended.target_status(), Some(TargetStatus::Partial));
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
fn a_failed_move_only_reports_partial_when_feedback_showed_displacement() {
    for (measured, expected) in [
        ([0.0, 0.0], TargetStatus::NotStarted),
        ([0.1, 0.0], TargetStatus::Partial),
    ] {
        let (mut core, _) = enabled_core();
        let movement = core.submit(mv([0.0, 0.0], [0.5, 0.0]), at(2.0)).unwrap();
        core.next_job().unwrap();
        core.begin_moving();
        core.observe(at(2.1), &measured, None, None);
        core.failed("The trajectory failed", at(2.2));
        let record = core.record(&movement.command_id).unwrap();
        assert_eq!(record.outcome(), Some(CommandOutcome::Failed));
        assert_eq!(record.target_status(), Some(expected));
    }
}

#[test]
fn a_stream_takes_goals_by_id_until_it_ends_and_then_says_so() {
    let (mut core, _) = enabled_core();
    let outside_limits = core.submit(
        Command::StreamJoints {
            gripper_angle: None,
            positions: vec![0.1, 3.0],
            joint_speed: None,
            stream_command_id: None,
        },
        at(2.0),
    );
    assert_eq!(reason(outside_limits), RejectionReason::InvalidTarget);
    let start = core
        .submit(
            Command::StreamJoints {
                gripper_angle: None,
                positions: vec![0.1, 2.0],
                joint_speed: Some(hebbian_contracts::JointSpeed::new(0.5).unwrap()),
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
    assert_eq!(stream_record_joints(&start), [0.1, 2.0]);
    assert_eq!(record_speed(&start), 0.5);
    for joint_speed in [Some(hebbian_contracts::JointSpeed::new(9.0).unwrap()), None] {
        let update = core
            .submit(
                Command::StreamJoints {
                    gripper_angle: None,
                    positions: vec![0.2, 0.0],
                    joint_speed,
                    stream_command_id: Some(start.command_id.clone()),
                },
                at(2.1),
            )
            .unwrap();
        assert_eq!(update.command_id, start.command_id);
        assert_eq!(stream_record_joints(&update), [0.2, 0.0]);
        let (stream_goal, _) = core.stream_goal(&start.command_id).unwrap();
        assert_eq!(
            stream_goal
                .joint_speed()
                .map(crate::motion_values::JointSpeed::radians_per_second),
            Some(record_speed(&start))
        );
    }
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
    assert_eq!(interrupted.achieved(), None);
    assert_eq!(interrupted.target_status(), None);
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
    assert_eq!(
        reason(core.submit(Command::ClearFault, at(3.5))),
        RejectionReason::Faulted
    );
    observe(&mut core, 3.4, Some("1: 80C"));
    assert!(
        core.submit(Command::ClearFault, at(3.5))
            .unwrap_err()
            .detail
            .contains("80C")
    );
    observe(&mut core, 3.4, None);
    let stale_after_clock_step = Now {
        unix_seconds: 3.5,
        ..at(9.0)
    };
    assert_eq!(
        reason(core.submit(Command::ClearFault, stale_after_clock_step)),
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
                angle: hebbian_contracts::JawAngle::try_from(0.5).unwrap(),
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
                angle: hebbian_contracts::JawAngle::try_from(0.2).unwrap(),
                joint_speed: None,
            },
            at(2.0),
        )
        .unwrap();
    let second = core
        .submit(
            Command::SetGripper {
                angle: hebbian_contracts::JawAngle::try_from(0.8).unwrap(),
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
    assert_eq!(target.angle.radians(), 0.8);
    assert_eq!(target.speed.radians_per_second(), 1.0);
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
            positions: vec![0.1],
            gripper_angle: Some(hebbian_contracts::JawAngle::try_from(0.5).unwrap()),
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
                gripper_angle: Some(hebbian_contracts::JawAngle::try_from(0.5).unwrap()),
                joint_speed: None,
                stream_command_id: None,
            },
            at(3.0),
        )
        .unwrap();
    assert_eq!(stream_record_joints(&accepted), [0.1, 0.0]);
    assert!(core.next_job().is_some());
    assert_eq!(core.take_gripper_target().unwrap().1.angle.radians(), 0.5);
}

#[test]
fn a_move_replaces_a_settling_one_from_the_measured_joints_and_reports_it_superseded() {
    let (mut core, _) = enabled_core();
    let settling = core.submit(mv([0.0, 0.0], [0.5, 0.0]), at(2.0)).unwrap();
    core.next_job().unwrap();
    core.begin_moving();
    assert_eq!(
        reason(core.submit(mv([0.0, 0.0], [0.1, 0.0]), at(2.5))),
        RejectionReason::ArmBusy
    );
    core.observe(at(3.0), &[0.45, 0.0], None, None);
    core.set_settling(true);
    let replacement = core.submit(mv([0.5, 0.0], [0.0, 0.3]), at(3.0)).unwrap();
    let superseded = core.record(&settling.command_id).unwrap();
    assert_eq!(superseded.outcome(), Some(CommandOutcome::Superseded));
    let errors = superseded.achieved().unwrap();
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
    core.observe(at(3.0), &[0.45, 0.0], None, None);
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
    core.observe(at(3.0), &[0.45, 0.0], None, None);
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
    core.observe(at(3.0), &[0.449, 0.0], None, None);
    core.motion_ended(CommandOutcome::StoppedShort, None, &[0.5, 0.0], at(3.0));
    let finished = core.record(&record.command_id).unwrap();
    assert_eq!(finished.target_status(), Some(TargetStatus::StoppedShort));
}

fn stopped_short_detail(target_position: Option<[f64; 3]>) -> Option<String> {
    let (mut core, _) = enabled_core();
    let mut pose_plan = plan([0.0, 0.0], [0.5, 0.0]);
    pose_plan.segments[0].target_position = target_position;
    let command = Command::Move {
        label: "move_to_pose".into(),
        plan: pose_plan,
        joint_speed: hebbian_contracts::JointSpeed::new(1.0).unwrap(),
        path_limited: None,
        pose_limited: None,
    };
    let record = core.submit(command, at(2.0)).unwrap();
    core.next_job().unwrap();
    core.begin_moving();
    core.observe(
        at(3.0),
        &[0.449, 0.0],
        Some(([0.2, 0.0, 0.0079], [0.0, 0.0, 0.0, 1.0])),
        None,
    );
    core.motion_ended(
        CommandOutcome::StoppedShort,
        Some("Stopped short; final joint errors (rad): [0.051, 0.000]".into()),
        &[0.5, 0.0],
        at(3.0),
    );
    core.record(&record.command_id)
        .unwrap()
        .detail()
        .map(str::to_owned)
}

#[test]
fn a_move_that_stopped_short_says_where_the_tool_rests_and_how_far_from_an_exact_target() {
    assert_eq!(
        stopped_short_detail(Some([0.2, 0.0, 0.0])).as_deref(),
        Some(
            "Stopped 7.9 mm short of the target and no longer closing on it, with the tool \
             at [200.0, 0.0, 7.9] mm; something is likely in the way, such as the table or \
             an object"
        )
    );
    assert_eq!(
        stopped_short_detail(None).as_deref(),
        Some(
            "Stopped short of the target and no longer closing on it, with the tool at \
             [200.0, 0.0, 7.9] mm; something is likely in the way, such as the table or an \
             object"
        )
    );
}

#[test]
fn malformed_motion_plans_are_refused_without_occupying_the_motion_channel() {
    let (mut core, _) = enabled_core();
    let mut malformed_plans = Vec::new();
    for duration in [f64::NAN, f64::INFINITY, -0.1] {
        let mut motion_plan = plan([0.0, 0.0], [0.1, 0.0]);
        motion_plan.segments[0].minimum_duration_seconds = Some(duration);
        malformed_plans.push(motion_plan);
    }
    let mut empty_plan = plan([0.0, 0.0], [0.1, 0.0]);
    empty_plan.segments.clear();
    malformed_plans.push(empty_plan);
    let mut wrong_joint_count = plan([0.0, 0.0], [0.1, 0.0]);
    wrong_joint_count.segments[0].corners[1].pop();
    malformed_plans.push(wrong_joint_count);
    for motion_plan in malformed_plans {
        let command = Command::Move {
            label: "invalid".into(),
            plan: motion_plan,
            joint_speed: hebbian_contracts::JointSpeed::new(0.5).unwrap(),
            path_limited: None,
            pose_limited: None,
        };
        assert_eq!(
            reason(core.submit(command, at(2.0))),
            RejectionReason::InvalidTarget
        );
        assert_eq!(core.mode(), ArmMode::Holding);
        assert!(core.next_job().is_none());
    }
    assert!(core.submit(mv([0.0, 0.0], [0.1, 0.0]), at(3.0)).is_ok());
}

proptest::proptest! {
    #![proptest_config(proptest::test_runner::Config::with_cases(96))]

    #[test]
    fn terminal_outcomes_survive_generated_command_and_recovery_sequences(
        operations in proptest::collection::vec((0_u8..11, -1.5_f64..1.5), 1..72)
    ) {
        use std::collections::BTreeMap;
        use proptest::{prop_assert, prop_assert_eq};
        let (mut core, _) = enabled_core();
        let mut known_ids: Vec<String> = Vec::new();
        let mut terminal_records: BTreeMap<String, CommandRecord> = BTreeMap::new();
        for (step, (operation, target)) in operations.into_iter().enumerate() {
            let seconds = f64::from(u32::try_from(step).unwrap()) + 2.0;
            let now = at(seconds);
            core.observe(at(seconds), &[0.0, 0.0], None, None);
            let command = match operation {
                0 => Some(mv([0.0, 0.0], [target, 0.0])),
                1 => Some(Command::SetGripper { angle: hebbian_contracts::JawAngle::try_from(target).unwrap(), joint_speed: None }),
                2 => Some(Command::Stop),
                3 => Some(Command::Disable),
                4 => Some(Command::Enable),
                5 => Some(Command::ClearFault),
                6 => Some(Command::Shutdown),
                _ => None,
            };
            if let Some(command) = command {
                let before: Vec<_> = known_ids.iter().filter_map(|id| core.record(id).cloned()).collect();
                let mode = core.mode();
                let active = core.active_command_id().map(str::to_owned);
                if let Ok(record) = core.submit(command, now) {
                    known_ids.push(record.command_id);
                } else {
                    prop_assert_eq!(core.mode(), mode);
                    prop_assert_eq!(core.active_command_id(), active.as_deref());
                    let after: Vec<_> = known_ids.iter().filter_map(|id| core.record(id).cloned()).collect();
                    prop_assert_eq!(after, before);
                }
            } else {
                match operation {
                    7 => { core.enter_fault("generated motor fault", &[], now); }
                    8 => { core.set_settling(true); }
                    9 => {
                        if core.mode() == ArmMode::Moving {
                            core.motion_ended(CommandOutcome::Completed, None, &[0.0, 0.0], now);
                        }
                    }
                    _ => {
                        if let Some(job) = core.next_job() {
                            match job {
                                Job::Enable { .. } => core.enabled(&[0.0, 0.0], now),
                                Job::Release { .. } => core.released(&[], now),
                                Job::Move { .. } | Job::Reach { .. } | Job::Stream { .. } => core.begin_moving(),
                                Job::ApproveRecovery { .. } => core.failed("not awaiting approval", now),
                            }
                        }
                    }
                }
            }
            for (id, completed) in &terminal_records {
                prop_assert_eq!(core.record(id), Some(completed));
            }
            for id in &known_ids {
                if let Some(record) = core.record(id) && record.outcome().is_some() {
                    terminal_records.insert(id.clone(), record.clone());
                }
            }
            if core.shutdown_requested() {
                prop_assert!(core.submit(mv([0.0, 0.0], [target, 0.0]), now).is_err());
            }
        }
    }

    #[test]
    fn invalid_combined_streams_do_not_replace_either_existing_target(
        joint_target in -2.0_f64..2.0,
        gripper_target in 0.0_f64..1.0,
        speed in 0.01_f64..2.0
    ) {
        use proptest::{prop_assert, prop_assert_eq};
        let (mut core, _) = enabled_core();
        let gripper = core.submit(Command::SetGripper { angle: hebbian_contracts::JawAngle::try_from(gripper_target).unwrap(), joint_speed: Some(hebbian_contracts::JointSpeed::new(speed).unwrap()) }, at(2.0)).unwrap();
        let stream = core.submit(Command::StreamJoints {
            positions: vec![0.25, 0.0], joint_speed: Some(hebbian_contracts::JointSpeed::new(speed).unwrap()), stream_command_id: None, gripper_angle: None,
        }, at(2.0)).unwrap();
        let previous_goal = core.stream_goal(&stream.command_id).unwrap().0.clone();
        let previous_record = core.record(&gripper.command_id).unwrap().clone();
        let invalid = Command::StreamJoints {
            positions: vec![joint_target], joint_speed: Some(hebbian_contracts::JointSpeed::new(speed).unwrap()),
            stream_command_id: Some(stream.command_id.clone()), gripper_angle: Some(hebbian_contracts::JawAngle::try_from(0.8).unwrap()),
        };
        prop_assert!(core.submit(invalid, at(3.0)).is_err());
        prop_assert_eq!(core.stream_goal(&stream.command_id).unwrap().0, &previous_goal);
        prop_assert_eq!(core.record(&gripper.command_id), Some(&previous_record));
        prop_assert_eq!(core.take_gripper_target().unwrap().1.angle.radians().to_bits(), gripper_target.to_bits());
    }
}

#[test]
fn only_the_measured_start_may_use_the_joint_recovery_tolerance() {
    let (mut core, _) = enabled_core();
    core.set_hold(&[2.02, 0.0]);
    assert!(core.submit(mv([0.0, 0.0], [1.5, 0.0]), at(2.0)).is_ok());
    let Some(Job::Move { plan, .. }) = core.next_job() else {
        panic!("An inward recovery move was admitted");
    };
    assert_eq!(plan.initial_corner(), &[2.02, 0.0]);
    core.motion_ended(CommandOutcome::Completed, None, &[1.5, 0.0], at(3.0));
    assert_eq!(
        reason(core.submit(mv([1.5, 0.0], [2.02, 0.0]), at(4.0))),
        RejectionReason::InvalidTarget
    );
    core.set_hold(&[2.04, 0.0]);
    assert_eq!(
        reason(core.submit(mv([0.0, 0.0], [1.5, 0.0]), at(5.0))),
        RejectionReason::InvalidTarget
    );
}

#[test]
fn a_running_stream_survives_completed_record_history_eviction() {
    let (mut core, _) = enabled_core();
    let stream = core
        .submit(
            Command::StreamJoints {
                positions: vec![0.1, 0.0],
                joint_speed: None,
                stream_command_id: None,
                gripper_angle: None,
            },
            at(2.0),
        )
        .unwrap();
    core.next_job().unwrap();
    core.begin_moving();
    for _ in 0..(RETAINED_RECORDS + 20) {
        core.submit(
            Command::SetGripper {
                angle: hebbian_contracts::JawAngle::try_from(0.5).unwrap(),
                joint_speed: None,
            },
            at(2.1),
        )
        .unwrap();
    }
    let updated = core
        .submit(
            Command::StreamJoints {
                positions: vec![0.2, 0.0],
                joint_speed: None,
                stream_command_id: Some(stream.command_id.clone()),
                gripper_angle: None,
            },
            at(2.2),
        )
        .unwrap();
    assert_eq!(updated.command_id, stream.command_id);
    assert_eq!(stream_record_joints(&updated), [0.2, 0.0]);
    core.motion_ended(CommandOutcome::Stopped, None, &[0.2, 0.0], at(2.3));
    assert_eq!(
        core.record(&stream.command_id).unwrap().outcome(),
        Some(CommandOutcome::Stopped)
    );
}

fn stream_record_joints(record: &hebbian_contracts::CommandRecord) -> &[f64] {
    let hebbian_contracts::RecordedCommand::JointStream { goal_joints, .. } = &record.command
    else {
        panic!("Expected a joint stream: {record:?}");
    };
    goal_joints
}

fn record_speed(record: &hebbian_contracts::CommandRecord) -> f64 {
    use hebbian_contracts::RecordedCommand;
    match record.command {
        RecordedCommand::Move { joint_speed, .. }
        | RecordedCommand::JointStream { joint_speed, .. }
        | RecordedCommand::Gripper { joint_speed, .. } => joint_speed,
        _ => panic!("Expected a command with a speed: {record:?}"),
    }
}

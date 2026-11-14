//! Deterministic servo-lag regressions: no wall clock, sockets or hardware.
use hebbian_contracts::{CornerPlan, Segment};
use hebbian_robot_use_server::config::ExecutorConfig;
use hebbian_robot_use_server::corner_path::{MotionSupervisor, Stage};

fn config() -> ExecutorConfig {
    ExecutorConfig::parse(
        r#"{
      "urdf":"unused.urdf", "driven_joints":["a","b"], "surface_guard":null,
      "motion":{"acceleration_limit":5.0,"jerk_limit":20.0,
      "settled_error_radians":0.04,"settle_dwell_seconds":0.3,"settle_timeout_seconds":8.0},
      "joint_count":2,"frequency_hertz":50.0,"loop_deadline_seconds":0.1,
      "lower_limits":[-2.0,-2.0],"upper_limits":[2.0,2.0],"joint_effort_limits":[10.0,10.0],
      "tracking_following_lag_seconds":0.05,"tracking_static_margin_radians":0.1,"max_joint_speed":0.6,"default_joint_speed":0.6,
      "stream_goal_timeout_seconds":0.5,"joint_limit_recovery_tolerance_radians":0.0349,"jaw_closing_axis":"x"
    }"#,
    )
    .unwrap()
}

fn plan() -> CornerPlan {
    CornerPlan {
        segments: vec![Segment {
            label: "target".into(),
            corners: vec![vec![0.0, 0.0], vec![0.6, -0.3]],
            minimum_duration_seconds: None,
            target_position: None,
            target_quaternion: None,
        }],
        peak_speed: 0.6,
    }
}

#[test]
fn a_planned_move_starts_without_an_initial_settle_dwell() {
    let config = config();
    let mut motion = MotionSupervisor::new(plan(), &config.motion, 2).unwrap();
    let reference = motion.step(&[0.0, 0.0], 0.02).unwrap();
    assert_eq!(motion.stage(), Stage::Moving);
    assert!(reference.positions[0] > 0.0);
}

#[test]
fn a_lagging_arm_finishes_without_repeated_stops_or_tracking_trips() {
    let config = config();
    for (lag_seconds, second_joint_lag) in [(0.05, 0.05), (0.10, 0.10), (0.02, 0.20)] {
        let mut motion = MotionSupervisor::new(plan(), &config.motion, 2).unwrap();
        let mut measured = vec![0.0; 2];
        let mut command = measured.clone();
        let mut previous_velocity = vec![0.0; 2];
        let mut previous_acceleration = vec![0.0; 2];
        let mut max_error: f64 = 0.0;
        let mut finished = false;
        let mut peak_speed: f64 = 0.0;
        for cycle in 0..2000 {
            for ((measured, command), lag) in measured
                .iter_mut()
                .zip(&command)
                .zip([lag_seconds, second_joint_lag])
            {
                *measured += (command - *measured) * (1.0 - (-0.02_f64 / lag).exp());
            }
            let reference = motion.step(&measured, 0.02).unwrap();
            peak_speed = peak_speed.max(reference.velocities[0].abs());
            for joint in 0..2 {
                assert!(
                    (reference.positions[0] + 2.0 * reference.positions[1]).abs() < 1e-9,
                    "retiming left the collision-checked straight path"
                );
                max_error = max_error.max((reference.positions[joint] - measured[joint]).abs());
                assert!(reference.velocities[joint].abs() <= 0.60001);
                assert!(reference.accelerations[joint].abs() <= 5.00001);
                if cycle > 0 {
                    assert!(
                        (reference.velocities[joint] - previous_velocity[joint]).abs()
                            <= 5.00001 * 0.02
                    );
                    assert!(
                        (reference.accelerations[joint] - previous_acceleration[joint]).abs()
                            <= 20.00001 * 0.02
                    );
                }
            }
            if motion.stage() == Stage::Moving
                && reference.positions[0] > 0.05
                && reference.positions[0] < 0.55
            {
                assert!(
                    reference.velocities[0] > 0.001,
                    "motion stopped mid-path lag={lag_seconds} cycle={cycle} reference={reference:?} measured={measured:?}"
                );
            }
            command = reference.positions;
            previous_velocity = reference.velocities;
            previous_acceleration = reference.accelerations;
            if motion.done() {
                assert!((measured[0] - 0.6).abs() <= config.motion.settled_error_radians);
                eprintln!(
                    "lag={lag_seconds:.2}s elapsed={:.2}s max_error={max_error:.4}rad peak_speed={peak_speed:.3}rad/s",
                    f64::from(cycle + 1) * 0.02
                );
                assert!(
                    peak_speed >= 0.59,
                    "the requested 0.6 rad/s ceiling was never reached"
                );
                finished = true;
                break;
            }
        }
        assert!(finished, "lag={lag_seconds}: the move never finished");
    }
}

#[test]
fn a_combined_group_finishes_at_one_shared_checkpoint() {
    let mut motion_limits = config().motion.clone();
    motion_limits.acceleration_limit = 20.0;
    motion_limits.jerk_limit = 200.0;
    motion_limits.settled_error_radians = 0.02;
    motion_limits.settle_dwell_seconds = 0.1;
    let mut plan = plan();
    plan.peak_speed = 1.0;
    plan.segments[0].corners = vec![vec![0.0; 4], vec![0.6, 0.0, 0.0, -0.3]];
    let mut motion = MotionSupervisor::new(plan, &motion_limits, 4).unwrap();
    let mut command = vec![0.0; 4];
    let mut measured = command.clone();
    for _ in 0..1000 {
        for (measured, command) in measured.iter_mut().zip(&command) {
            *measured += (command - *measured) * (1.0 - (-0.5_f64).exp());
        }
        command = motion.step(&measured, 0.01).unwrap().positions;
        if motion.done() {
            return;
        }
    }
    panic!(
        "group never finished: {:?} {:?}",
        motion.stage(),
        motion.reference()
    );
}

#[test]
fn a_loaded_joint_can_finish_within_the_configured_tracking_tolerance() {
    let config = config();
    let mut motion = MotionSupervisor::new(plan(), &config.motion, 2).unwrap();
    let mut measured = vec![0.0; 2];
    let mut command = measured.clone();
    for _ in 0..2000 {
        for (measured, command) in measured.iter_mut().zip(&command) {
            let error: f64 = command - *measured;
            let driven_error = error.signum() * (error.abs() - 0.035).max(0.0);
            *measured += driven_error * (1.0 - (-0.4_f64).exp());
        }
        command = motion.step(&measured, 0.02).unwrap().positions;
        if motion.done() {
            return;
        }
    }
    panic!("a joint with 0.035 rad of load deflection never completed");
}

/// Settle at the end of the path while the arm measures its second joint
/// `error_at(t)` past the target, `t` seconds into the settling wait. Returns
/// how long the wait lasted and the finished supervisor.
fn settle_with_error(error_at: impl Fn(f64) -> f64) -> (f64, MotionSupervisor) {
    let config = config();
    let mut motion = MotionSupervisor::new(plan(), &config.motion, 2).unwrap();
    let mut measured = vec![0.0; 2];
    let mut settling_seconds = 0.0;
    for _ in 0..4000 {
        let reference = motion.step(&measured, 0.02).unwrap();
        if motion.done() {
            break;
        }
        if motion.settling_at_end() {
            measured = vec![
                reference.positions[0],
                reference.positions[1] + error_at(settling_seconds),
            ];
            settling_seconds += 0.02;
        } else {
            measured = reference.positions;
        }
    }
    (settling_seconds, motion)
}

#[test]
fn an_arm_that_rests_outside_the_tolerance_stops_short_after_the_settle_timeout() {
    let timeout = config().motion.settle_timeout_seconds;
    let (seconds, motion) = settle_with_error(|_| 0.06);
    assert!(motion.done() && motion.stopped_short());
    assert!(
        (timeout - 0.1..=timeout + 0.1).contains(&seconds),
        "waited {seconds} s for a {timeout} s settle timeout"
    );
}

#[test]
fn an_arm_that_converges_within_the_settle_timeout_settles_normally() {
    let (seconds, motion) = settle_with_error(|t| (0.08 - 0.05 * t).max(0.0));
    assert!(motion.done() && !motion.stopped_short());
    assert!(
        seconds < config().motion.settle_timeout_seconds,
        "settled after {seconds} s"
    );
}

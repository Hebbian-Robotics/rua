#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    reason = "This integration-test target may abort on failed setup or unexpected outcomes."
)]

use hebbian_contracts::{CornerPlan, Segment};
use hebbian_robot_use_core::config::ControllerConfig;
use hebbian_robot_use_core::corner_path::{MotionSupervisor, Stage};

fn config() -> ControllerConfig {
    ControllerConfig::parse(
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
            pass_through: None,
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

#[test]
fn variable_cycle_intervals_preserve_motion_limits_and_minimum_duration() {
    let configuration = config();
    for minimum_duration in [None, Some(3.0)] {
        let mut motion_plan = plan();
        motion_plan.segments[0].minimum_duration_seconds = minimum_duration;
        let mut motion = MotionSupervisor::new(motion_plan, &configuration.motion, 2).unwrap();
        let mut command = vec![0.0; 2];
        let mut previous_velocity = vec![0.0; 2];
        let mut previous_acceleration = vec![0.0; 2];
        let mut elapsed_seconds = 0.0;
        for cycle in 0..2000 {
            let interval = [0.001, 0.007, 0.019, 0.031, 0.012][cycle % 5];
            let reference = motion.step(&command, interval).unwrap();
            elapsed_seconds += interval;
            assert!((reference.positions[0] + 2.0 * reference.positions[1]).abs() < 1e-9);
            for joint in 0..2 {
                assert!(reference.velocities[joint].abs() <= 0.60001);
                assert!(reference.accelerations[joint].abs() <= 5.00001);
                assert!(
                    (reference.velocities[joint] - previous_velocity[joint]).abs()
                        <= 5.00001 * interval
                );
                assert!(
                    (reference.accelerations[joint] - previous_acceleration[joint]).abs()
                        <= 20.00001 * interval
                );
            }
            if motion.stage() != Stage::Moving
                && let Some(minimum) = minimum_duration
            {
                assert!(
                    elapsed_seconds >= minimum - 1e-9,
                    "settling at {elapsed_seconds:.12}s before minimum {minimum}s; reference {reference:?}"
                );
            }
            command = reference.positions;
            previous_velocity = reference.velocities;
            previous_acceleration = reference.accelerations;
            if motion.done() {
                break;
            }
        }
        assert!(motion.done(), "motion never finished");
        assert!((command[0] - 0.6).abs() < 1e-9);
        assert!((command[1] + 0.3).abs() < 1e-9);
    }
}

#[test]
fn a_constant_target_preserves_the_continuous_jerk_limited_profile() {
    let motion_limits = config().motion.clone();
    let mut motion_plan = plan();
    motion_plan.segments[0].corners = vec![vec![0.0; 5], vec![10.0, -5.0, 3.0, -2.0, 1.0]];
    let mut motion = MotionSupervisor::new(motion_plan, &motion_limits, 5).unwrap();
    let maximum_velocity = 0.6_f64;
    let jerk = motion_limits.jerk_limit;
    let ramp_seconds = (maximum_velocity / jerk).sqrt();
    assert!(jerk * ramp_seconds < motion_limits.acceleration_limit);
    let mut command = vec![0.0; 5];
    let mut elapsed_seconds = 0.0;
    for _ in 0..1000 {
        let interval = 0.001;
        let reference = motion.step(&command, interval).unwrap();
        elapsed_seconds += interval;
        // WHY: This long section is still accelerating or cruising: integrate the
        // two constant-jerk ramps analytically, independently of Ruckig.
        let (position, velocity, acceleration) = if elapsed_seconds <= ramp_seconds {
            (
                jerk * elapsed_seconds.powi(3) / 6.0,
                jerk * elapsed_seconds.powi(2) / 2.0,
                jerk * elapsed_seconds,
            )
        } else if elapsed_seconds <= 2.0 * ramp_seconds {
            let after_peak = elapsed_seconds - ramp_seconds;
            (
                jerk * ramp_seconds.powi(3) / 6.0
                    + jerk * ramp_seconds.powi(2) / 2.0 * after_peak
                    + jerk * ramp_seconds / 2.0 * after_peak.powi(2)
                    - jerk * after_peak.powi(3) / 6.0,
                jerk * ramp_seconds.powi(2) / 2.0 + jerk * ramp_seconds * after_peak
                    - jerk * after_peak.powi(2) / 2.0,
                jerk * (ramp_seconds - after_peak),
            )
        } else {
            (
                maximum_velocity * (elapsed_seconds - ramp_seconds),
                maximum_velocity,
                0.0,
            )
        };
        for (joint, scale) in [1.0, -0.5, 0.3, -0.2, 0.1].into_iter().enumerate() {
            assert!((reference.positions[joint] - position * scale).abs() < 1e-10);
            assert!((reference.velocities[joint] - velocity * scale).abs() < 1e-10);
            assert!((reference.accelerations[joint] - acceleration * scale).abs() < 1e-10);
        }
        command = reference.positions;
    }
}

#[test]
fn pass_through_keeps_velocity_at_a_waypoint_and_settles_at_the_goal() {
    let config = config();
    let mut path = plan();
    let mut final_section = path.segments[0].clone();
    final_section.corners = vec![vec![0.6, -0.3], vec![1.0, -0.1]];
    path.segments[0].pass_through = Some(hebbian_contracts::PassThrough {
        gripper_angle: None,
    });
    path.segments.push(final_section);
    let mut motion = MotionSupervisor::new(path, &config.motion, 2).unwrap();
    let mut measured = vec![0.0; 2];
    let mut crossed = false;
    for _ in 0..2000 {
        let reference = motion.step(&measured, 0.01).unwrap();
        if measured[0] < 0.6 && reference.positions[0] >= 0.6 {
            assert!(reference.velocities[0] > 0.01);
            assert_eq!(motion.stage(), Stage::Moving);
            crossed = true;
        }
        measured = reference.positions;
        if motion.done() {
            break;
        }
    }
    assert!(crossed && motion.done());
    assert_eq!(measured, vec![1.0, -0.1]);
}

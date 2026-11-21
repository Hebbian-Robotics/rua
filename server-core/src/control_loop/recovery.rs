use std::time::Duration;

use crate::config::ExecutorConfig;
use crate::limits::max_joint_deviation;
use crate::motion_values::MeasuredPositions;
use crate::plant::{Feedback, RecoveryObservation};
use hebbian_contracts::ArmRecoveryIssue;

pub(super) fn recovery_positions<'a>(
    config: &ExecutorConfig,
    feedback: &'a Feedback,
) -> anyhow::Result<MeasuredPositions<'a>> {
    config.recovery_limits().clamp(&feedback.positions)?;
    feedback.measured_positions(config)
}

pub(super) enum RecoveryHold {
    Waiting,
    Verified,
    Refused(ArmRecoveryIssue),
}

pub(super) fn assess_recovery_hold(
    config: &ExecutorConfig,
    held: &Feedback,
    observation: &RecoveryObservation,
    elapsed: Duration,
) -> RecoveryHold {
    let gripper_stable = match (held.gripper_angle, observation.feedback.gripper_angle) {
        (Some(before), Some(after)) => {
            (before - after).abs() <= config.motion.settled_error_radians
        }
        (None, None) => true,
        _ => false,
    };
    let stable = observation.feedback.measured_positions(config).is_ok()
        && !observation.motor_torque.is_empty()
        && observation.motor_torque.iter().all(|(_, enabled)| *enabled)
        && observation.health_issue.is_none()
        && max_joint_deviation(&held.positions, &observation.feedback.positions)
            <= config.motion.settled_error_radians
        && gripper_stable;
    if !stable {
        return RecoveryHold::Refused(ArmRecoveryIssue {
            motors_without_torque: observation
                .motor_torque
                .iter()
                .filter_map(|(id, enabled)| (!enabled).then_some(*id))
                .collect(),
            detail: observation
                .health_issue
                .clone()
                .unwrap_or_else(|| "The arm did not remain at the recovery hold".into()),
        });
    }
    if elapsed.as_secs_f64() >= config.motion.settle_dwell_seconds {
        RecoveryHold::Verified
    } else {
        RecoveryHold::Waiting
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_recovery_hold_requires_stable_valid_feedback_and_energized_healthy_motors() {
        let mut settings = ExecutorConfig::parse(crate::config::TEST_CONFIG)
            .unwrap()
            .into_input();
        settings.motion.settle_dwell_seconds = 0.1;
        settings.motion.settled_error_radians = 0.02;
        let config = ExecutorConfig::try_from(settings).unwrap();
        let held = Feedback {
            positions: vec![0.0, 0.0],
            gripper_angle: Some(0.5),
            ..Feedback::default()
        };
        let healthy = RecoveryObservation {
            feedback: held.clone(),
            motor_torque: vec![(1, true), (2, true)],
            health_issue: None,
        };
        assert!(matches!(
            assess_recovery_hold(&config, &held, &healthy, Duration::from_millis(99)),
            RecoveryHold::Waiting
        ));
        assert!(matches!(
            assess_recovery_hold(&config, &held, &healthy, Duration::from_millis(100)),
            RecoveryHold::Verified
        ));
        for (positions, gripper_angle, motor_torque, health_issue) in [
            (vec![0.1, 0.0], Some(0.5), vec![(1, true), (2, true)], None),
            (vec![0.0, 0.0], Some(0.6), vec![(1, true), (2, true)], None),
            (vec![0.0, 0.0], None, vec![(1, true), (2, true)], None),
            (
                vec![f64::NAN, 0.0],
                Some(0.5),
                vec![(1, true), (2, true)],
                None,
            ),
            (vec![0.0], Some(0.5), vec![(1, true), (2, true)], None),
            (vec![0.0, 0.0], Some(0.5), vec![], None),
            (vec![0.0, 0.0], Some(0.5), vec![(1, true), (2, false)], None),
            (
                vec![0.0, 0.0],
                Some(0.5),
                vec![(1, true), (2, true)],
                Some("overheated"),
            ),
        ] {
            let observed = RecoveryObservation {
                feedback: Feedback {
                    positions,
                    gripper_angle,
                    ..held.clone()
                },
                motor_torque,
                health_issue: health_issue.map(str::to_owned),
            };
            assert!(matches!(
                assess_recovery_hold(&config, &held, &observed, Duration::from_secs(1)),
                RecoveryHold::Refused(_)
            ));
        }
    }
}

use crate::config::ControllerConfig;
use std::fmt::Write as _;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trip(pub String);

impl Trip {
    fn because(reason: impl Into<String>) -> Self {
        Self(reason.into())
    }
}

#[must_use]
pub(crate) fn is_finite_joint_vector(values: &[f64], joint_count: usize) -> bool {
    values.len() == joint_count && values.iter().all(|value| value.is_finite())
}

#[must_use]
pub(crate) fn max_joint_deviation(left: &[f64], right: &[f64]) -> f64 {
    left.iter()
        .zip(right)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0, f64::max)
}

#[derive(Debug)]
pub(crate) enum TrackingFailure {
    InvalidFeedback(&'static str),
    ExcessLag {
        joint: usize,
        error: f64,
        allowed: f64,
        commanded_velocity: f64,
    },
}

impl TrackingFailure {
    pub(crate) fn into_trip(
        self,
        config: &ControllerConfig,
        feedback: &crate::plant::Feedback,
        motion_speed: Option<(f64, f64)>,
    ) -> Trip {
        match self {
            Self::InvalidFeedback(detail) => Trip::because(detail),
            Self::ExcessLag {
                joint,
                error,
                allowed,
                commanded_velocity,
            } => {
                let name = config
                    .driven_joints
                    .get(joint)
                    .map_or("unknown", String::as_str);
                let mut detail = format!(
                    "Feedback tracking error on joint {joint} ({name}) was {error:.3} rad, past the {allowed:.3} rad allowance; commanded {commanded_velocity:.2} rad/s"
                );
                if let Some(measured) = feedback
                    .velocities
                    .get(joint)
                    .filter(|value| value.is_finite())
                {
                    let _ = write!(detail, ", measured {measured:.2} rad/s");
                }
                if let Some((planned_peak, fraction)) = motion_speed {
                    let _ = write!(
                        detail,
                        "; planned peak {planned_peak:.2} rad/s, adaptive speed request {:.0}%",
                        fraction * 100.0
                    );
                }
                Trip::because(detail)
            }
        }
    }
}

pub(crate) fn validated_tracking_error(
    config: &ControllerConfig,
    measured: &[f64],
    commanded: &[f64],
    commanded_velocities: &[f64],
    interval_seconds: f64,
) -> Result<(), TrackingFailure> {
    if !is_finite_joint_vector(measured, config.joint_count) {
        return Err(TrackingFailure::InvalidFeedback("Invalid feedback joints"));
    }
    if !(interval_seconds.is_finite()
        && (0.0..=config.loop_deadline_seconds).contains(&interval_seconds))
    {
        return Err(TrackingFailure::InvalidFeedback(
            "Invalid feedback interval",
        ));
    }
    // WHY: A policy sets each target up to its lead ahead of the measured joint on purpose,
    // so that much error is its normal command and says nothing about the arm; on the real
    // arm, which answers a few cycles late, the full lead is still there at zero command speed.
    let policy_target_lead = config
        .policy
        .as_ref()
        .map_or(0.0, |policy| policy.max_target_lead_radians);
    for (joint, ((measured, commanded), velocity)) in measured
        .iter()
        .zip(commanded)
        .zip(commanded_velocities)
        .enumerate()
    {
        let error = (measured - commanded).abs();
        // INVARIANT: A blocked joint falls further behind every cycle and passes this:
        // the guard is also the stall check for an arm commanded to move.
        let allowed = tracking_allowance_radians(
            config.tracking_following_lag_seconds,
            config.tracking_static_margin_radians,
            velocity.abs(),
        ) + policy_target_lead;
        if error > allowed {
            return Err(TrackingFailure::ExcessLag {
                joint,
                error,
                allowed,
                commanded_velocity: *velocity,
            });
        }
    }
    Ok(())
}

#[must_use]
pub(crate) fn tracking_allowance_radians(
    following_lag_seconds: f64,
    static_margin_radians: f64,
    speed: f64,
) -> f64 {
    speed * following_lag_seconds + static_margin_radians
}

pub(crate) fn check_effort(config: &ControllerConfig, efforts: &[f64]) -> Result<(), Trip> {
    if efforts
        .iter()
        .zip(&config.joint_effort_limits)
        .any(|(effort, limit)| effort.abs() > *limit)
    {
        return Err(Trip::because(format!("Effort limit: {efforts:?}")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> ControllerConfig {
        ControllerConfig::parse(crate::config::TEST_CONFIG).unwrap()
    }

    #[test]
    fn the_tracking_allowance_grows_with_commanded_speed() {
        let config = config();
        let at_rest = [0.0, 0.0];
        let margin = config.tracking_static_margin_radians;
        let lag = config.tracking_following_lag_seconds;
        let past_margin = [margin + 0.01, 0.0];
        let trip = validated_tracking_error(&config, &[0.0, 0.0], &past_margin, &at_rest, 0.01);
        let detail = trip
            .unwrap_err()
            .into_trip(
                &config,
                &crate::plant::Feedback {
                    velocities: vec![0.03, 0.0],
                    ..Default::default()
                },
                Some((1.2, 0.5)),
            )
            .0;
        assert!(detail.contains("joint 0 (a)"), "{detail}");
        assert!(
            detail.contains("commanded 0.00 rad/s, measured 0.03 rad/s"),
            "{detail}"
        );
        assert!(
            detail.contains("planned peak 1.20 rad/s, adaptive speed request 50%"),
            "{detail}"
        );
        let fast_enough = [0.01 / lag + 0.1, 0.0];
        assert!(
            validated_tracking_error(&config, &[0.0, 0.0], &past_margin, &fast_enough, 0.01)
                .is_ok()
        );
    }

    #[test]
    fn a_policy_arm_may_trail_its_command_by_the_policy_target_lead_at_rest() {
        let lead = 0.12;
        let mut arm: serde_json::Value = serde_json::from_str(crate::config::TEST_CONFIG).unwrap();
        arm["driven_joints"] = serde_json::json!(["a", "b", "c", "d", "e"]);
        arm["joint_count"] = serde_json::json!(5);
        arm["lower_limits"] = serde_json::json!([-2.0, -2.0, -2.0, -2.0, -2.0]);
        arm["upper_limits"] = serde_json::json!([2.0, 2.0, 2.0, 2.0, 2.0]);
        arm["joint_effort_limits"] = serde_json::json!([10.0, 10.0, 10.0, 10.0, 10.0]);
        let without_policy = ControllerConfig::parse(&arm.to_string()).unwrap();
        arm["policy"] = serde_json::json!({
            "reach_deadline_seconds": 8.0, "settle_dwell_seconds": 0.1,
            "settled_velocity_radians_per_second": 0.15, "preview_lag_seconds": 0.05,
            "onnx": "unused.onnx",
            "checkpoint_sha256": "", "max_target_lead_radians": lead, "tool_link": "tool",
            "position_tolerance_meters": 0.02, "orientation_tolerance_radians": 0.1,
        });
        let with_policy = ControllerConfig::parse(&arm.to_string()).unwrap();
        let margin = with_policy.tracking_static_margin_radians;
        let measured = [0.0; 5];
        let at_rest = [0.0; 5];
        let trips = |config: &ControllerConfig, error: f64| {
            let commanded = [error, 0.0, 0.0, 0.0, 0.0];
            validated_tracking_error(config, &measured, &commanded, &at_rest, 0.01).is_err()
        };
        assert!(trips(&without_policy, margin + lead - 0.01));
        assert!(!trips(&with_policy, margin + lead - 0.01));
        assert!(trips(&with_policy, margin + lead + 0.01));
    }

    #[test]
    fn malformed_feedback_and_intervals_trip_instead_of_panicking() {
        let config = config();
        assert!(
            validated_tracking_error(&config, &[f64::NAN, 0.0], &[0.0, 0.0], &[0.0, 0.0], 0.01)
                .is_err()
        );
        assert!(validated_tracking_error(&config, &[0.0], &[0.0, 0.0], &[0.0, 0.0], 0.01).is_err());
        assert!(
            validated_tracking_error(&config, &[0.0, 0.0], &[0.0, 0.0], &[0.0, 0.0], 0.5).is_err()
        );
    }

    #[test]
    fn an_effort_past_a_joints_limit_trips() {
        let config = config();
        assert!(check_effort(&config, &[9.0, -9.0]).is_ok());
        assert!(check_effort(&config, &[0.0, -11.0]).is_err());
    }
}

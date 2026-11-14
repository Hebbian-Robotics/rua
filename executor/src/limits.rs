//! The per-cycle checks on feedback: finite values, a sane interval, tracking
//! error and effort. A failed check is a `Trip`: the arm is still controllable,
//! so the loop stops it and holds where it is.

use crate::config::ExecutorConfig;

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

/// Reject malformed feedback, then enforce the speed-scaled per-joint tracking
/// limit. `commanded_velocities` is the rate of change of the command the
/// feedback is compared with.
///
/// # Errors
/// A `Trip` when a joint trails its command by more than it is allowed.
pub(crate) fn validated_tracking_error(
    config: &ExecutorConfig,
    measured: &[f64],
    commanded: &[f64],
    commanded_velocities: &[f64],
    interval_seconds: f64,
) -> Result<(), Trip> {
    if !is_finite_joint_vector(measured, config.joint_count) {
        return Err(Trip::because("Invalid feedback joints"));
    }
    if !(interval_seconds.is_finite()
        && (0.0..=config.loop_deadline_seconds).contains(&interval_seconds))
    {
        return Err(Trip::because("Invalid feedback interval"));
    }
    for (joint, ((measured, commanded), velocity)) in measured
        .iter()
        .zip(commanded)
        .zip(commanded_velocities)
        .enumerate()
    {
        let error = (measured - commanded).abs();
        // A blocked joint falls further behind every cycle and passes this:
        // the guard is also the stall check for an arm commanded to move.
        let allowed = tracking_allowance_radians(
            config.tracking_following_lag_seconds,
            config.tracking_static_margin_radians,
            velocity.abs(),
        );
        if error > allowed {
            return Err(Trip::because(format!(
                "Feedback tracking error on joint {joint} was {error:.3} rad, past the {allowed:.3} rad allowed at {:.2} rad/s",
                velocity.abs()
            )));
        }
    }
    Ok(())
}

/// How far a joint may trail a command moving at `speed`: the servos follow a
/// moving command with a lag, so the allowance grows with commanded speed on
/// top of the static margin allowed at rest.
#[must_use]
pub(crate) fn tracking_allowance_radians(
    following_lag_seconds: f64,
    static_margin_radians: f64,
    speed: f64,
) -> f64 {
    speed * following_lag_seconds + static_margin_radians
}

/// # Errors
/// A `Trip` when any joint's effort is past its limit.
pub(crate) fn check_effort(config: &ExecutorConfig, efforts: &[f64]) -> Result<(), Trip> {
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

    fn config() -> ExecutorConfig {
        ExecutorConfig::parse(crate::config::TEST_CONFIG).unwrap()
    }

    #[test]
    fn the_tracking_allowance_grows_with_commanded_speed() {
        let config = config();
        let at_rest = [0.0, 0.0];
        let margin = config.tracking_static_margin_radians;
        let lag = config.tracking_following_lag_seconds;
        let past_margin = [margin + 0.01, 0.0];
        let trip = validated_tracking_error(&config, &[0.0, 0.0], &past_margin, &at_rest, 0.01);
        assert!(trip.unwrap_err().0.contains("tracking error"));
        let fast_enough = [0.01 / lag + 0.1, 0.0];
        assert!(
            validated_tracking_error(&config, &[0.0, 0.0], &past_margin, &fast_enough, 0.01)
                .is_ok()
        );
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

//! Joint recovery and executor motion-speed policy.
pub(crate) use hebbian_robot_geometry::joint_limits::{OutsideRecovery, clamp_within_recovery};

/// A requested joint speed that is not a positive, finite number.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct InvalidSpeed;

impl std::fmt::Display for InvalidSpeed {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("A joint speed must be positive and finite")
    }
}

/// The speed a motion runs at: the one asked for, else the default, and never
/// above the arm's maximum.
///
/// # Errors
/// The requested speed is not positive and finite.
pub(crate) fn motion_speed(
    requested: Option<f64>,
    default: f64,
    maximum: f64,
) -> Result<f64, InvalidSpeed> {
    match requested {
        Some(speed) if !(speed.is_finite() && speed > 0.0) => Err(InvalidSpeed),
        requested => Ok(requested.unwrap_or(default).min(maximum)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pose_within_the_tolerance_is_clamped_in_and_one_past_it_is_refused() {
        let (lower, upper) = ([-1.0, -1.0], [1.0, 1.0]);
        assert_eq!(
            clamp_within_recovery(&[1.03, -1.03], &lower, &upper, 0.035),
            Ok(vec![1.0, -1.0])
        );
        assert_eq!(
            clamp_within_recovery(&[0.0, -1.04], &lower, &upper, 0.035),
            Err(OutsideRecovery {
                joint: 1,
                value: -1.04
            })
        );
    }

    #[test]
    fn an_over_speed_request_runs_at_the_maximum_and_a_nonsense_one_is_refused() {
        assert_eq!(motion_speed(Some(9.0), 1.0, 2.0), Ok(2.0));
        assert_eq!(motion_speed(None, 1.0, 2.0), Ok(1.0));
        assert_eq!(motion_speed(Some(0.0), 1.0, 2.0), Err(InvalidSpeed));
        assert_eq!(motion_speed(Some(f64::NAN), 1.0, 2.0), Err(InvalidSpeed));
    }
}

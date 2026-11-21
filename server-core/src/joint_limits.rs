#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct InvalidSpeed;

impl std::fmt::Display for InvalidSpeed {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("A joint speed must be positive and finite")
    }
}

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

pub(crate) fn joint_target_limit_error(
    positions: &[f64],
    lower_limits: &[f64],
    upper_limits: &[f64],
) -> Option<String> {
    positions
        .iter()
        .zip(lower_limits.iter().zip(upper_limits))
        .enumerate()
        .find(|(_, (position, (lower, upper)))| position < lower || position > upper)
        .map(|(index, (position, (lower, upper)))| {
            format!("Joint {index} target {position} rad is outside [{lower}, {upper}] rad")
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_over_speed_request_runs_at_the_maximum_and_a_nonsense_one_is_refused() {
        assert_eq!(motion_speed(Some(9.0), 1.0, 2.0), Ok(2.0));
        assert_eq!(motion_speed(None, 1.0, 2.0), Ok(1.0));
        assert_eq!(motion_speed(Some(0.0), 1.0, 2.0), Err(InvalidSpeed));
        assert_eq!(motion_speed(Some(f64::NAN), 1.0, 2.0), Err(InvalidSpeed));
    }
}

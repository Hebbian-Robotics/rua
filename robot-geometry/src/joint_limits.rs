#[derive(Debug, Clone, PartialEq)]
pub enum RecoveryRefusal {
    WrongJointCount { expected: usize, got: usize },
    NotFinite { joint: usize },
    OutsideRecovery { joint: usize, value: f64 },
}

impl std::fmt::Display for RecoveryRefusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WrongJointCount { expected, got } => {
                write!(formatter, "Expected {expected} joints, got {got}")
            }
            Self::NotFinite { joint } => write!(formatter, "Joint {joint} is not finite"),
            Self::OutsideRecovery { joint, value } => {
                write!(
                    formatter,
                    "Joint {joint} at {value:.3} rad is outside its limits (beyond recovery tolerance)"
                )
            }
        }
    }
}

impl std::error::Error for RecoveryRefusal {}

#[derive(Debug, Clone)]
pub struct JointRecoveryLimits {
    lower: Vec<f64>,
    upper: Vec<f64>,
    tolerance: f64,
}

impl JointRecoveryLimits {
    pub fn new(lower: &[f64], upper: &[f64], tolerance: f64) -> anyhow::Result<Self> {
        anyhow::ensure!(
            !lower.is_empty() && lower.len() == upper.len(),
            "Joint limits need matching nonempty lower and upper bounds"
        );
        anyhow::ensure!(
            lower
                .iter()
                .zip(upper)
                .all(|(lower, upper)| lower.is_finite() && upper.is_finite() && lower < upper),
            "Every joint needs finite limits, with the lower below the upper"
        );
        anyhow::ensure!(
            tolerance.is_finite() && tolerance >= 0.0,
            "Joint recovery tolerance must be finite and not negative"
        );
        Ok(Self {
            lower: lower.to_vec(),
            upper: upper.to_vec(),
            tolerance,
        })
    }

    #[must_use]
    pub fn lower(&self) -> &[f64] {
        &self.lower
    }

    #[must_use]
    pub fn upper(&self) -> &[f64] {
        &self.upper
    }

    #[must_use]
    pub fn tolerance(&self) -> f64 {
        self.tolerance
    }

    pub fn clamp(&self, joints: &[f64]) -> Result<Vec<f64>, RecoveryRefusal> {
        if joints.len() != self.lower.len() {
            return Err(RecoveryRefusal::WrongJointCount {
                expected: self.lower.len(),
                got: joints.len(),
            });
        }
        joints
            .iter()
            .zip(self.lower.iter().zip(&self.upper))
            .enumerate()
            .map(|(joint, (value, (lower, upper)))| {
                if !value.is_finite() {
                    Err(RecoveryRefusal::NotFinite { joint })
                } else if *value < lower - self.tolerance || *value > upper + self.tolerance {
                    Err(RecoveryRefusal::OutsideRecovery {
                        joint,
                        value: *value,
                    })
                } else {
                    Ok(value.clamp(*lower, *upper))
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::{JointRecoveryLimits, RecoveryRefusal};

    #[test]
    fn invalid_bounds_and_tolerances_cannot_become_recovery_limits() {
        for (lower, upper, tolerance) in [
            (vec![], vec![], 0.0),
            (vec![-1.0], vec![1.0, 2.0], 0.0),
            (vec![1.0], vec![1.0], 0.0),
            (vec![2.0], vec![1.0], 0.0),
            (vec![f64::NAN], vec![1.0], 0.0),
            (vec![-1.0], vec![f64::INFINITY], 0.0),
            (vec![-1.0], vec![1.0], -0.1),
            (vec![-1.0], vec![1.0], f64::NAN),
            (vec![-1.0], vec![1.0], f64::INFINITY),
        ] {
            assert!(JointRecoveryLimits::new(&lower, &upper, tolerance).is_err());
        }
    }

    #[test]
    fn malformed_measurements_are_refused_instead_of_truncated_or_clamped() {
        let limits = JointRecoveryLimits::new(&[-1.0, -1.0], &[1.0, 1.0], 0.035).unwrap();
        for positions in [vec![], vec![0.0], vec![0.0, 0.0, 0.0]] {
            assert_eq!(
                limits.clamp(&positions),
                Err(RecoveryRefusal::WrongJointCount {
                    expected: 2,
                    got: positions.len(),
                })
            );
        }
        for value in [f64::NAN, f64::NEG_INFINITY, f64::INFINITY] {
            assert_eq!(
                limits.clamp(&[0.0, value]),
                Err(RecoveryRefusal::NotFinite { joint: 1 })
            );
        }
        assert_eq!(limits.clamp(&[1.03, -1.03]), Ok(vec![1.0, -1.0]));
        assert_eq!(
            limits.clamp(&[0.0, -1.04]),
            Err(RecoveryRefusal::OutsideRecovery {
                joint: 1,
                value: -1.04
            })
        );
    }

    #[test]
    fn recovery_is_bounded_and_idempotent_across_joint_ranges() {
        // WHY: Binary fractions exercise both tolerance endpoints without decimal
        // rounding deciding which side of the boundary a measurement occupies.
        for scale in [0.001, 1.0, 1000.0] {
            let lower = [-scale, 2.0 * scale];
            let upper = [scale, 4.0 * scale];
            let tolerance = scale / 8.0;
            let limits = JointRecoveryLimits::new(&lower, &upper, tolerance).unwrap();
            for sample in -16..=16 {
                let offset = f64::from(sample) * tolerance / 16.0;
                let measured = [upper[0] + offset, lower[1] - offset];
                let recovered = limits.clamp(&measured).unwrap();
                for joint in 0..2 {
                    assert!(recovered[joint] >= lower[joint] && recovered[joint] <= upper[joint]);
                    assert!(
                        (recovered[joint] - measured[joint]).abs() <= tolerance + scale * 1e-12
                    );
                }
                assert_eq!(limits.clamp(&recovered).unwrap(), recovered);
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct JointTarget<Positions>(Positions);

impl<Positions: AsRef<[f64]>> JointTarget<Positions> {
    pub(crate) fn parse(
        positions: Positions,
        expected: usize,
        lower_limits: &[f64],
        upper_limits: &[f64],
    ) -> Result<Self, JointTargetError> {
        let values = positions.as_ref();
        if values.len() != expected {
            return Err(JointTargetError::Count {
                expected,
                actual: values.len(),
            });
        }
        if lower_limits.len() != expected || upper_limits.len() != expected {
            return Err(JointTargetError::InconsistentLimits);
        }
        for (index, (&position, (&lower, &upper))) in values
            .iter()
            .zip(lower_limits.iter().zip(upper_limits))
            .enumerate()
        {
            if !position.is_finite() {
                return Err(JointTargetError::NonFinite { index });
            }
            if !(lower..=upper).contains(&position) {
                return Err(JointTargetError::OutsideLimits {
                    index,
                    position,
                    lower,
                    upper,
                });
            }
        }
        Ok(Self(positions))
    }

    pub(crate) fn as_slice(&self) -> &[f64] {
        self.0.as_ref()
    }
}

#[derive(Debug)]
pub(crate) enum JointTargetError {
    Count {
        expected: usize,
        actual: usize,
    },
    InconsistentLimits,
    NonFinite {
        index: usize,
    },
    OutsideLimits {
        index: usize,
        position: f64,
        lower: f64,
        upper: f64,
    },
}

impl std::fmt::Display for JointTargetError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Count { expected, actual } => write!(
                formatter,
                "Expected {expected} joint positions, got {actual}"
            ),
            Self::InconsistentLimits => {
                formatter.write_str("Arm limits do not match its joint count")
            }
            Self::NonFinite { index } => write!(formatter, "Joint {index} target must be finite"),
            Self::OutsideLimits {
                index,
                position,
                lower,
                upper,
            } => write!(
                formatter,
                "Joint {index} target {position} rad is outside [{lower}, {upper}] rad"
            ),
        }
    }
}

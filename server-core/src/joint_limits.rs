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

#[cfg(test)]
mod tests {
    use super::{JointTarget, JointTargetError};
    use hebbian_contracts::StationCommand;

    #[test]
    fn a_wire_target_at_the_joint_limit_is_admitted() {
        let upper_limit = 1.709_203_884_727_313_9_f64;
        for (target, should_be_admitted) in [(upper_limit, true), (upper_limit.next_up(), false)] {
            let command = StationCommand::StreamJoints {
                joints: vec![target],
                max_joint_speed: None,
                gripper_opening: None,
            };
            let encoded = serde_json::to_vec(&command).unwrap();
            let StationCommand::StreamJoints { joints, .. } =
                serde_json::from_slice(&encoded).unwrap()
            else {
                panic!("Expected a joint stream command");
            };

            let result = JointTarget::parse(joints, 1, &[-2.0], &[upper_limit]);
            if should_be_admitted {
                assert_eq!(result.unwrap().as_slice(), &[upper_limit]);
            } else {
                assert!(matches!(
                    result,
                    Err(JointTargetError::OutsideLimits { .. })
                ));
            }
        }
    }
}

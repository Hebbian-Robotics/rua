pub(crate) use hebbian_contracts::JointSpeed;

#[derive(Debug, Clone, Copy)]
pub(crate) struct MeasuredPositions<'a>(&'a [f64]);

impl<'a> MeasuredPositions<'a> {
    pub(crate) fn new(positions: &'a [f64], expected: usize) -> anyhow::Result<Self> {
        anyhow::ensure!(
            positions.len() == expected && positions.iter().all(|value| value.is_finite()),
            "Invalid feedback joints: expected {expected} finite positions"
        );
        Ok(Self(positions))
    }

    pub(crate) fn as_slice(self) -> &'a [f64] {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GroupId(String);

impl From<String> for GroupId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for GroupId {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl GroupId {
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use hebbian_contracts::{InvalidSpeed, JointSpeed};

    #[test]
    fn an_over_speed_request_runs_at_the_maximum_and_a_nonsense_one_is_refused() {
        assert_eq!(
            JointSpeed::for_request(Some(JointSpeed::new(9.0).unwrap()), 1.0, 2.0)
                .map(JointSpeed::radians_per_second),
            Ok(2.0)
        );
        assert_eq!(
            JointSpeed::for_request(None, 1.0, 2.0).map(JointSpeed::radians_per_second),
            Ok(1.0)
        );
        assert_eq!(JointSpeed::new(0.0), Err(InvalidSpeed));
        assert_eq!(JointSpeed::new(f64::NAN), Err(InvalidSpeed));
    }
}

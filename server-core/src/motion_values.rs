#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct JointAngle(f64);

impl JointAngle {
    pub(crate) fn new(radians: f64) -> anyhow::Result<Self> {
        anyhow::ensure!(radians.is_finite(), "A joint angle must be finite");
        Ok(Self(radians))
    }

    pub(crate) fn radians(self) -> f64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct JointSpeed(f64);

impl JointSpeed {
    pub(crate) fn new(radians_per_second: f64) -> anyhow::Result<Self> {
        anyhow::ensure!(
            radians_per_second.is_finite() && radians_per_second > 0.0,
            "A joint speed must be positive and finite"
        );
        Ok(Self(radians_per_second))
    }

    pub(crate) fn radians_per_second(self) -> f64 {
        self.0
    }
}

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

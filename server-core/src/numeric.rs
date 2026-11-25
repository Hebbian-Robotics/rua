use std::time::{Duration, Instant};

use anyhow::{Context, ensure};

#[must_use]
#[allow(clippy::cast_precision_loss)]
pub(crate) fn count(value: usize) -> f64 {
    value as f64
}

pub(crate) fn positive_clock_duration(seconds: f64, field: &str) -> anyhow::Result<Duration> {
    let duration = Duration::try_from_secs_f64(seconds)
        .with_context(|| format!("{field} must fit in a positive duration"))?;
    ensure!(!duration.is_zero(), "{field} rounds to a zero duration");
    ensure!(
        Instant::now().checked_add(duration).is_some(),
        "{field} exceeds the supported clock range"
    );
    Ok(duration)
}

#[must_use]
pub(crate) fn is_finite_joint_vector(values: &[f64], joint_count: usize) -> bool {
    values.len() == joint_count && values.iter().all(|value| value.is_finite())
}

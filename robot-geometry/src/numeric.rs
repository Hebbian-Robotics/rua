//! Numeric conversions used by the executor.

/// A count of items (rest points, path samples) as a float, far below 2^52.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub(crate) fn count(value: usize) -> f64 {
    value as f64
}

#[must_use]
// INVARIANT: Counts of samples and points are far below 2^52, so this cast is exact.
#[allow(clippy::cast_precision_loss)]
pub(crate) fn count(value: usize) -> f64 {
    value as f64
}

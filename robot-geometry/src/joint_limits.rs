//! Joint-limit recovery shared by inverse kinematics and executor admission.

/// A pose with a joint farther outside its limits than the recovery tolerance.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OutsideRecovery {
    pub joint: usize,
    pub value: f64,
}

/// A measured pose clamped into the joint limits, when no joint is outside them
/// by more than `tolerance`, as one resting at an end stop may be. This is the
/// one check of a starting pose: enabling, the reference filter's start and the
/// pose solver's seed all go through it.
///
/// Callers supply equally sized finite slices, ordered limits and a finite,
/// nonnegative tolerance. `IkSolver::new` and the executor configuration boundary
/// establish these invariants before this operation runs in the control path.
///
/// # Errors
/// The first joint outside its limits by more than the tolerance.
pub fn clamp_within_recovery(
    joints: &[f64],
    lower: &[f64],
    upper: &[f64],
    tolerance: f64,
) -> Result<Vec<f64>, OutsideRecovery> {
    joints
        .iter()
        .zip(lower.iter().zip(upper))
        .enumerate()
        .map(|(joint, (value, (lower, upper)))| {
            if *value < lower - tolerance || *value > upper + tolerance {
                Err(OutsideRecovery {
                    joint,
                    value: *value,
                })
            } else {
                Ok(value.clamp(*lower, *upper))
            }
        })
        .collect()
}

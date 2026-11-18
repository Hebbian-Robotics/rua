// INVARIANT: The control loop owns ArmCore; readers reach it only through its inbox. Publish record changes in mutation order so state and event history agree.

mod admission;
mod motion_admission;
pub(crate) use motion_admission::{AdmittedPlan, MotionScope, MoveDecision};
mod loop_api;
mod state;
mod types;

#[cfg(test)]
mod tests;

pub use state::ArmCore;
pub(crate) use state::new_identifier;
pub use types::*;

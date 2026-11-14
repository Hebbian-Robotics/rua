//! Learned motion execution shared by the control loop and preview.
//!
//! The current model adapter is [`crate::so101::policy`]. Observation layouts,
//! action decoding and embodiment requirements belong there. This module owns
//! execution/arrival state, not hardware writes or final protective checks.
//! Current execution remains single-arm; a coordinated embodiment will need an
//! explicit joint-group scope, not one independent policy instance per arm.

mod reach;
mod runtime;

pub(crate) use reach::{ActiveReach, ReachStatus};
pub(crate) use runtime::PolicyRuntime;

//! The arm's state machine and its command records. One `ArmCore` per arm decides which commands are
//! admitted, keeps their queue and records, and reports each change as a logged
//! event, so the state and its history cannot disagree.
//!
//! It is plain logic with no plant and no clock of its own: the control loop
//! drives it (`next_job`, `finish_active`, `enter_fault`, ...) and owns it: the
//! server's reader threads reach it only through the loop's inbox (`handoff`).
//! Every change to a record publishes it as it happens, so the event log's order
//! is the order things happened.

mod admission;
mod loop_api;
mod state;
mod types;

#[cfg(test)]
mod tests;

pub use state::ArmCore;
pub(crate) use state::new_identifier;
pub use types::*;

//! Station-facing API and client coordination, outside the motor control loop.
//!
//! Hosts start the HTTP server and supply run telemetry through this facade.
//! Routes, submission retries, action sequences and event replay remain private.
//! Commands and feedback cross through [`crate::control_link`]; the control loop
//! does not depend on this module's HTTP or client-session implementation.

mod action_sequences;
mod commands;
mod events;
mod http;
mod interlocks;
mod state;
mod submissions;
mod telemetry;

pub use http::{HttpServer, default_viewer_directory, trace_directories};
pub use telemetry::{DiscardTelemetry, RunTelemetry, RunUpdates};

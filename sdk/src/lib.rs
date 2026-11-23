//! An async client for a Hebbian robot-use station.
//!
//! [`Station::connect`] opens a resumable HTTP event stream and submits commands
//! over HTTP with operation identifiers. Acquire
//! the lease with [`Station::acquire_lease`]; the [`LeaseGuard`] it returns carries every
//! command that moves an arm, while power, stop and reads stay on [`Station`].
//! Wire types come from `hebbian-rua-contracts`, re-exported as [`contracts`].

#![warn(clippy::missing_errors_doc, clippy::missing_panics_doc)]

mod connection;
mod error;
mod http;
#[path = "generated/http_routes.rs"]
mod http_routes;
mod response_format;
mod robot_tools;
mod sequence_images;
mod station;
#[path = "generated/station_commands.rs"]
mod station_commands;

pub use hebbian_contracts as contracts;

pub use connection::RunUpdate;
pub use error::{Error, Result};
pub use http::CameraJpeg;
pub use response_format::{
    ConciseOptions, concise_output, concise_response, concise_response_with, model_response,
};
pub use robot_tools::{RobotContext, RobotToolCall, RobotToolOutcome, RobotToolResult, RobotTools};
pub use sequence_images::{SequenceImage, SequenceImageBoundary, SequenceImageHistory};
pub use station::{
    CommandOptions, ConnectOptions, DEFAULT_MEDIA_SERVER, DEFAULT_SERVER, LeaseGuard, LeaseRequest,
    Station,
};

mod tool_operations;

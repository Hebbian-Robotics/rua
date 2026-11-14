//! The station beside the control loop: run telemetry, recordings, the lease's
//! HTTP routes and `LiveKit` access on the `station_api` HTTP server.

pub mod controller_process;
pub mod controller_supervisor;
pub mod lease_routes;
pub mod media_status;
pub mod openapi;
pub mod recording;
pub mod run_telemetry;
pub mod web;

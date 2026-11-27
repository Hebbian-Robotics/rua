#![deny(clippy::wildcard_enum_match_arm)]
pub mod action_step;
pub mod arm;
mod compiler;
pub mod config;
pub mod control_link;
pub mod control_loop;
pub mod controller_bridge;
pub mod controller_endpoint;
pub mod controller_service;
pub mod controller_transport;
pub mod corner_path;
pub(crate) mod cycle;
pub(crate) mod gripper;
pub mod guard;
pub(crate) mod handoff;
pub mod ik;
pub(crate) mod joint_limits;
pub mod kinematics;
pub mod lease;
pub(crate) mod limits;
mod motion_guards;
pub(crate) mod motion_values;
pub(crate) mod numeric;
pub mod orientation;
pub mod planning;
pub mod plant;
pub mod policy;
pub mod preview;
pub mod profile;
pub(crate) mod reference_filter;
pub mod robot;
pub mod self_collision;
pub mod sim_plant;
pub mod so101;
pub mod station_api;
pub(crate) mod surface_guard;
pub(crate) mod sync;
pub(crate) mod trace;

pub fn running_executable_for_child() -> std::io::Result<std::path::PathBuf> {
    // WHY: On Linux, the process image survives replacement of the on-disk binary.
    #[cfg(target_os = "linux")]
    {
        Ok(std::path::PathBuf::from("/proc/self/exe"))
    }
    #[cfg(not(target_os = "linux"))]
    {
        std::env::current_exe()
    }
}

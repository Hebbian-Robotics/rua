//! Robot geometry and inverse kinematics independent of station configuration and control policy.
#[cfg(feature = "model")]
pub mod ik;
#[cfg(feature = "model")]
pub mod joint_limits;
#[cfg(feature = "model")]
pub mod kinematics;
#[cfg(feature = "model")]
mod numeric;
pub mod outcomes;
#[cfg(feature = "model")]
pub mod robot;
#[cfg(feature = "model")]
pub mod self_collision;

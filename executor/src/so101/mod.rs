//! SO-101 embodiment integration: per-arm settings, `LeRobot` calibration and
//! the STS3215 plant adapter and trained policy contract. Station orchestration
//! and shared learned-policy execution stay outside
//! this module. Existing Feetech profile keys and runtime config types are retained.

pub mod calibration;
pub mod discovery;
pub mod driver;
pub(crate) mod executor_config;
pub mod policy;
pub mod registry;
pub mod settings;

/// The joints the SO-101 policy was trained on, in order.
pub(crate) const SO101_JOINT_NAMES: [&str; 5] = [
    "shoulder_pan",
    "shoulder_lift",
    "elbow_flex",
    "wrist_flex",
    "wrist_roll",
];

//! Adapt host gripper settings to the independent SO-101 configuration loader.

use super::settings::FeetechSettings;
use crate::config::FeetechConfig;
use crate::profile::GripperSettings;
use std::path::Path;

pub(crate) fn feetech_bus(
    gripper_settings: Option<&GripperSettings>,
    settings: &FeetechSettings,
    arm: &str,
    calibration_root: &Path,
) -> anyhow::Result<FeetechConfig> {
    hebbian_so101::config::load_arm_config(
        gripper_settings.map(|gripper| gripper.force_fraction),
        settings,
        arm,
        calibration_root,
    )
}

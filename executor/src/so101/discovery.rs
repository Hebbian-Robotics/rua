use super::SO101_JOINT_NAMES;
use crate::profile::{Driver, RobotProfile, load_robot_profile};
use anyhow::{Context, bail, ensure};
pub use hebbian_so101::discovery::{
    ControllerMatch, ControllerSerial, UsbController, discover, match_controllers,
};
use hebbian_so101::setup::{ArmSetup, ModelVariant};
use std::path::{Path, PathBuf};

pub fn default_config_directory() -> anyhow::Result<PathBuf> {
    if let Some(directory) =
        std::env::var_os("HEBBIAN_SO101_CONFIG_DIRECTORY").filter(|value| !value.is_empty())
    {
        return Ok(PathBuf::from(directory));
    }
    let config_home = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map_or_else(
            || std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")),
            |directory| Some(PathBuf::from(directory)),
        )
        .context("Set HEBBIAN_SO101_CONFIG_DIRECTORY or HOME")?;
    Ok(config_home.join("rua").join("so101-arm-configs"))
}

#[derive(Debug)]
pub struct RegisteredController {
    pub controller: UsbController,
    pub directory: PathBuf,
}

pub fn select_controller(
    matches: &[ControllerMatch],
    requested: Option<&ControllerSerial>,
) -> anyhow::Result<RegisteredController> {
    let selected = hebbian_so101::discovery::select_controller(matches, requested)?;
    Ok(RegisteredController {
        controller: selected.controller,
        directory: selected.directory,
    })
}

impl RegisteredController {
    pub fn load_profile(&self, profile_directory: &Path) -> anyhow::Result<RobotProfile> {
        let setup_path = self.directory.join("setup.toml");
        if setup_path.exists() {
            ensure!(
                !self.directory.join("profile.toml").exists(),
                "Both setup.toml and profile.toml exist; migrate the profile before using the native setup"
            );
            let setup = ArmSetup::load(&setup_path, self.controller.serial.as_str())?;
            let base = match setup.model_variant {
                ModelVariant::Stock => "so101-manual",
                ModelVariant::SoftGripperCamera => "so101-soft-gripper-camera-manual",
            };
            let default_profile = load_robot_profile(profile_directory, base)?;
            let overrides_path = self.directory.join("rua.toml");
            let mut profile = if overrides_path.exists() {
                let overrides_path = std::fs::canonicalize(overrides_path)?;
                let overridden = load_robot_profile(
                    profile_directory,
                    overrides_path
                        .to_str()
                        .context("RUA override path is not UTF-8")?,
                )?;
                ensure!(
                    overridden.urdf == default_profile.urdf
                        && overridden.arms == default_profile.arms
                        && overridden.joint_names == default_profile.joint_names,
                    "RUA overrides must extend the matching SO-101 model profile"
                );
                let overridden_settings = overridden
                    .feetech()
                    .context("RUA overrides require a Feetech driver")?;
                let default_settings = default_profile
                    .feetech()
                    .context("SO-101 base profile requires a Feetech driver")?;
                ensure!(
                    overridden_settings.motor_ids == default_settings.motor_ids
                        && overridden_settings.gripper_motor_id
                            == default_settings.gripper_motor_id
                        && overridden_settings.passive_motor_ids
                            == default_settings.passive_motor_ids,
                    "RUA overrides cannot change the SO-101 motor mapping"
                );
                overridden
            } else {
                default_profile
            };
            profile.name = format!("so101-{}", setup.serial);
            let Driver::Feetech(settings) = &mut profile.driver else {
                bail!("SO-101 base profile must use the Feetech driver")
            };
            settings
                .ports
                .insert("follower".to_owned(), self.controller.port.clone());
            settings
                .calibration_ids
                .insert("follower".to_owned(), format!("so101_{}", setup.serial));
            settings.joint_signs = setup.alignment.joint_signs.into();
            settings.joint_offsets_radians = setup.alignment.joint_offsets_radians.into();
            settings.gripper_sign = setup.alignment.gripper_sign;
            settings.gripper_offset_radians = setup.alignment.gripper_offset_radians;
            settings.expected_position_gains = setup.servos.position_gains;
            settings.expected_position_gains_by_motor = setup.servos.position_gains_by_motor;
            settings.expected_return_delay_time = setup.servos.return_delay_time;
            settings.validate_profile_mapping(
                &profile.arms,
                profile.joint_names.len(),
                profile.gripper.is_some(),
            )?;
            let gripper = profile
                .gripper
                .as_mut()
                .context("SO-101 base profile has no gripper")?;
            gripper.closed_position_radians = setup.gripper.closed_position_radians;
            gripper.open_position_radians = setup.gripper.open_position_radians;
            return Ok(profile);
        }
        let profile_path = std::fs::canonicalize(self.directory.join("profile.toml"))
            .context("Registered arm has no readable profile.toml")?;
        let mut profile = load_robot_profile(
            profile_directory,
            profile_path.to_str().context("Profile path is not UTF-8")?,
        )?;
        ensure!(
            profile.joint_names == SO101_JOINT_NAMES && profile.arms == ["follower"],
            "Automatic SO-101 selection requires a single follower with SO-101 joint names"
        );
        let Driver::Feetech(settings) = &mut profile.driver else {
            bail!("Registered SO-101 profile must use the Feetech driver")
        };
        let calibration_id = format!("so101_{}", self.controller.serial.as_str());
        ensure!(
            settings.calibration_ids.get("follower") == Some(&calibration_id),
            "Registered calibration ID must be {calibration_id}"
        );
        settings
            .ports
            .insert("follower".to_owned(), self.controller.port.clone());
        Ok(profile)
    }
}

#[cfg(test)]
mod tests;

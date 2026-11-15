//! Host profile adapter and defaults for SO-101 controller discovery.

use super::SO101_JOINT_NAMES;
use crate::profile::{Driver, RobotProfile, load_robot_profile};
use anyhow::{Context, bail, ensure};
pub use hebbian_so101::discovery::{
    ControllerMatch, ControllerSerial, UsbController, discover, match_controllers,
};
use std::path::{Path, PathBuf};

/// The local checkout location, configurable without embedding a private remote.
///
/// # Errors
/// Neither a config directory nor the user's home directory is available.
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

/// A controller selected for the host's inherited profile loader.
#[derive(Debug)]
pub struct RegisteredController {
    pub controller: UsbController,
    pub directory: PathBuf,
}

/// Select one registered controller without opening its serial device.
///
/// # Errors
/// The requested controller is absent or cannot be selected unambiguously.
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
    /// Load the arm's inherited profile and replace its machine-specific port.
    ///
    /// # Errors
    /// The entry is malformed or is not a single SO-101 follower profile with
    /// the serial-derived calibration ID used by this registry.
    pub fn load_profile(&self, profile_directory: &Path) -> anyhow::Result<RobotProfile> {
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

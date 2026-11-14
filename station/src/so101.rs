//! SO-101 USB discovery and profile selection shared by station commands.

use std::path::{Path, PathBuf};

use anyhow::{bail, ensure};
use hebbian_robot_use_server::profile::{RobotProfile, load_robot_profile};
use hebbian_robot_use_server::so101::calibration::lerobot_calibration_root;
use hebbian_robot_use_server::so101::discovery::{self, ControllerSerial};
use hebbian_robot_use_server::so101::registry::{CalibrationInstallation, install_calibration};

#[derive(clap::Args, Default)]
pub(crate) struct SelectionArguments {
    /// Local SO-101 config checkout containing arms/<USB-serial>/{profile.toml,calibration.json}.
    /// Default: `$HEBBIAN_SO101_CONFIG_DIRECTORY`, else `$XDG_CONFIG_HOME/rua/so101-arm-configs`.
    #[arg(long)]
    so101_config_directory: Option<PathBuf>,
    /// Select one connected USB controller when several arms are registered.
    #[arg(long)]
    so101_serial: Option<String>,
}

impl SelectionArguments {
    fn directory(&self) -> anyhow::Result<PathBuf> {
        self.so101_config_directory
            .clone()
            .map_or_else(discovery::default_config_directory, Ok)
    }

    pub(crate) fn require_explicit_profile(&self) -> anyhow::Result<()> {
        ensure!(
            self.so101_config_directory.is_none() && self.so101_serial.is_none(),
            "SO-101 selection options require --profile auto"
        );
        Ok(())
    }

    pub(crate) fn load_profile(
        &self,
        name: &str,
        profile_directory: &Path,
    ) -> anyhow::Result<RobotProfile> {
        if name != "auto" {
            self.require_explicit_profile()?;
            return load_robot_profile(profile_directory, name);
        }
        let matches = discovery::discover(&self.directory()?)?;
        let requested = self
            .so101_serial
            .as_deref()
            .map(ControllerSerial::parse)
            .transpose()?;
        let selected = discovery::select_controller(&matches, requested.as_ref())?;
        let profile = selected.load_profile(profile_directory)?;
        let settings = profile
            .feetech()
            .expect("Selected SO-101 profile uses Feetech");
        let installation = install_calibration(
            &selected.directory.join("calibration.json"),
            settings,
            "follower",
            &lerobot_calibration_root(),
        )?;
        match installation {
            CalibrationInstallation::Installed(path) => {
                eprintln!("Installed calibration {}", path.display());
            }
            CalibrationInstallation::Unchanged(_) => {}
            CalibrationInstallation::Conflict(path) => bail!(
                "Local calibration {} differs from the registry. Reconcile the arm's calibration before using automatic selection; existing data was preserved",
                path.display()
            ),
        }
        eprintln!(
            "Selected SO-101 {} on {}",
            selected.controller.serial.as_str(),
            selected.controller.port
        );
        Ok(profile)
    }
}

pub(crate) fn print_discovered(config_directory: Option<&Path>) -> anyhow::Result<()> {
    let directory = config_directory
        .map(Path::to_owned)
        .map_or_else(discovery::default_config_directory, Ok)?;
    let matches = discovery::discover(&directory)?;
    println!("{}", serde_json::to_string_pretty(&matches)?);
    Ok(())
}

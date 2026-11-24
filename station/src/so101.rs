use std::path::{Path, PathBuf};

use anyhow::{Context, bail, ensure};
use hebbian_rua_core::profile::{RobotProfile, load_robot_profile};
use hebbian_rua_core::so101::calibration::{calibrated_joint_limits, lerobot_calibration_root};
use hebbian_rua_core::so101::discovery::{self, ControllerSerial};
use hebbian_rua_core::so101::registry::{CalibrationInstallation, install_calibration};

use crate::commands::SelectionArguments;

impl SelectionArguments {
    fn directory(&self) -> anyhow::Result<PathBuf> {
        self.so101_config_directory
            .clone()
            .map_or_else(discovery::default_config_directory, Ok)
    }

    fn require_explicit_profile(&self) -> anyhow::Result<()> {
        ensure!(
            self.so101_config_directory.is_none() && self.so101_serial.is_none(),
            "SO-101 selection options require automatic profile selection"
        );
        Ok(())
    }

    pub(crate) fn load_profile(
        &self,
        name: &str,
        profile_directory: &Path,
        no_physics_stand_in: bool,
    ) -> anyhow::Result<RobotProfile> {
        if name != "auto" {
            let profile = load_robot_profile(profile_directory, name)?;
            // WHY: On a stand-in or a simulator, `--so101-serial` names the registered arm to
            // mirror, not a controller to select.
            if !no_physics_stand_in && profile.feetech().is_some() {
                self.require_explicit_profile()?;
            }
            return Ok(profile);
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
            .context("Selected SO-101 profile has no Feetech settings")?;
        let installation = install_calibration(
            &selected.directory.join("calibration.json"),
            settings,
            &profile.arms[0],
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

impl SelectionArguments {
    // WHY: A stand-in or simulated arm stops where the registered arm `--so101-serial` names
    // does, not at the URDF's generic limits. Its calibration goes under `calibration_root`,
    // never the operator's LeRobot calibration directory.
    pub(crate) fn mirrored_arm_joint_limits(
        &self,
        profile: &RobotProfile,
        profile_directory: &Path,
        calibration_root: &Path,
    ) -> anyhow::Result<Option<Vec<(f64, f64)>>> {
        let Some(serial) = self.so101_serial.as_deref() else {
            ensure!(
                self.so101_config_directory.is_none(),
                "--so101-config-directory without hardware needs --so101-serial to name the arm to mirror"
            );
            return Ok(None);
        };
        let serial = ControllerSerial::parse(serial)?;
        let directory = self.directory()?.join("arms").join(serial.as_str());
        ensure!(
            directory.is_dir(),
            "SO-101 {} is not registered in {}",
            serial.as_str(),
            directory.display()
        );
        ensure!(
            profile.arms.len() == 1,
            "Mirroring a registered SO-101 needs a profile with one arm"
        );
        let registered = discovery::RegisteredController {
            controller: discovery::UsbController {
                serial,
                // WHY: A stand-in or simulated arm opens no serial port; the registered
                // profile only records it.
                port: "mirrored".to_owned(),
            },
            directory,
        };
        let registered_profile = registered.load_profile(profile_directory)?;
        ensure!(
            registered_profile.urdf == profile.urdf,
            "SO-101 {} is set up as {}, but profile {} is for {}",
            registered.controller.serial.as_str(),
            registered_profile.urdf.display(),
            profile.name,
            profile.urdf.display()
        );
        let settings = registered_profile
            .feetech()
            .context("Registered SO-101 profile has no Feetech settings")?;
        let arm_name = &registered_profile.arms[0];
        install_calibration(
            &registered.directory.join("calibration.json"),
            settings,
            arm_name,
            calibration_root,
        )?;
        Ok(Some(calibrated_joint_limits(
            settings,
            arm_name,
            calibration_root,
        )?))
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

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use hebbian_rua_core::profile::load_robot_profile_with_fragments;

    use crate::commands::SelectionArguments;

    struct Registry(PathBuf);

    impl Registry {
        fn with_arm(name: &str, model_variant: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "station-stand-in-arm-{name}-{}",
                std::process::id()
            ));
            let arm = root.join("arms").join("ARM_A");
            std::fs::create_dir_all(&arm).unwrap();
            std::fs::write(
                arm.join("setup.toml"),
                format!("serial = \"ARM_A\"\nmodel_variant = \"{model_variant}\"\n[motor_ids]\nshoulder_pan = 1\nshoulder_lift = 2\nelbow_flex = 3\nwrist_flex = 4\nwrist_roll = 5\ngripper = 6\n"),
            )
            .unwrap();
            let joints = [
                "shoulder_pan",
                "shoulder_lift",
                "elbow_flex",
                "wrist_flex",
                "wrist_roll",
                "gripper",
            ];
            let calibration: serde_json::Map<String, serde_json::Value> = joints
                .into_iter()
                .zip(1..=6)
                .map(|(joint, motor_id)| {
                    (
                        joint.to_owned(),
                        serde_json::json!({"id": motor_id, "drive_mode": 0, "homing_offset": 0, "range_min": 1023, "range_max": 3071}),
                    )
                })
                .collect();
            std::fs::write(
                arm.join("calibration.json"),
                serde_json::to_string(&calibration).unwrap(),
            )
            .unwrap();
            Self(root)
        }

        fn selection(&self) -> SelectionArguments {
            SelectionArguments {
                so101_config_directory: Some(self.0.clone()),
                so101_serial: Some("ARM_A".to_owned()),
            }
        }
    }

    impl Drop for Registry {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn profiles() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("profiles")
    }

    #[test]
    fn a_stand_in_arm_stops_where_the_registered_arm_does() {
        let registry = Registry::with_arm("mirrored", "soft-gripper-camera");
        let profile = load_robot_profile_with_fragments(
            &profiles(),
            "so101-sim",
            &["so101-soft-gripper-camera-assembly"],
        )
        .unwrap();
        let limits = registry
            .selection()
            .mirrored_arm_joint_limits(&profile, &profiles(), &registry.0.join("calibration"))
            .unwrap()
            .unwrap();
        for (lower, upper) in limits {
            assert!((lower + std::f64::consts::FRAC_PI_2).abs() < 1e-3);
            assert!((upper - std::f64::consts::FRAC_PI_2).abs() < 1e-3);
        }
    }

    #[test]
    fn an_arm_set_up_as_another_model_cannot_be_mirrored() {
        let registry = Registry::with_arm("stock", "stock");
        let profile = load_robot_profile_with_fragments(
            &profiles(),
            "so101-sim",
            &["so101-soft-gripper-camera-assembly"],
        )
        .unwrap();
        let error = registry
            .selection()
            .mirrored_arm_joint_limits(&profile, &profiles(), &registry.0.join("calibration"))
            .unwrap_err();
        assert!(error.to_string().contains("is set up as"), "{error:#}");
    }
}

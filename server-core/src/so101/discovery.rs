use crate::profile::{Driver, RobotProfile, load_robot_profile_with_fragments};
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

#[derive(Default, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ArmMotionOverrides {
    motion: MotionOverrides,
    trajectory: TrajectoryOverrides,
}

#[derive(Default, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
struct MotionOverrides {
    default_joint_speed: Option<f64>,
    max_joint_speed: Option<f64>,
}

#[derive(Default, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
struct TrajectoryOverrides {
    acceleration_limit: Option<f64>,
    jerk_limit: Option<f64>,
}

fn apply_motion_overrides(path: &Path, profile: &mut RobotProfile) -> anyhow::Result<()> {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).with_context(|| format!("Cannot read {}", path.display())),
    };
    let overrides: ArmMotionOverrides =
        toml::from_str(&content).with_context(|| format!("Cannot parse {}", path.display()))?;
    let maximum = overrides
        .motion
        .max_joint_speed
        .unwrap_or(profile.motion.max_joint_speed);
    let default = overrides
        .motion
        .default_joint_speed
        .unwrap_or_else(|| profile.motion.default_joint_speed.min(maximum));
    let acceleration = overrides
        .trajectory
        .acceleration_limit
        .unwrap_or(profile.trajectory.acceleration_limit);
    let jerk = overrides
        .trajectory
        .jerk_limit
        .unwrap_or(profile.trajectory.jerk_limit);
    ensure!(
        [maximum, default, acceleration, jerk]
            .iter()
            .all(|rate| rate.is_finite() && *rate > 0.0),
        "{}: motion rates must be positive and finite",
        path.display(),
    );
    ensure!(
        default <= maximum,
        "{}: default_joint_speed cannot exceed max_joint_speed",
        path.display()
    );
    profile.motion.default_joint_speed = default;
    profile.motion.max_joint_speed = maximum;
    profile.trajectory.acceleration_limit = acceleration;
    profile.trajectory.jerk_limit = jerk;
    Ok(())
}

impl RegisteredController {
    pub fn load_profile(&self, profile_directory: &Path) -> anyhow::Result<RobotProfile> {
        let setup_path = self.directory.join("setup.toml");
        let setup =
            ArmSetup::load(&setup_path, self.controller.serial.as_str()).with_context(|| {
                format!(
                    "Cannot load {}; run SO-101 setup for this arm",
                    setup_path.display()
                )
            })?;
        setup
            .motor_ids
            .validate_calibration_mapping(&self.directory.join("calibration.json"))?;
        let assembly: &[_] = match setup.model_variant {
            ModelVariant::Stock => &[],
            ModelVariant::SoftGripperCamera => &["so101-soft-gripper-camera-assembly"],
        };
        let mut profile = load_robot_profile_with_fragments(profile_directory, "so101", assembly)?;
        // WHY: Give native hardware setup time for bounded serial fault cleanup before guard takeover.
        profile.guard.heartbeat_timeout_seconds = 3.0;
        apply_motion_overrides(&self.directory.join("motion.toml"), &mut profile)?;
        profile.name = format!("so101-{}", setup.serial);
        let arm_name = "arm".to_owned();
        profile.arms = vec![arm_name.clone()];
        let Driver::Feetech(settings) = &mut profile.driver else {
            bail!("SO-101 base profile must use the Feetech driver")
        };
        settings.ports = [(arm_name.clone(), self.controller.port.clone())].into();
        settings.calibration_ids = [(arm_name, format!("so101_{}", setup.serial))].into();
        let (joint_motor_ids, gripper_motor_id) = setup.motor_ids.motor_ids_in_joint_order();
        settings.motor_ids = joint_motor_ids.into();
        settings.gripper_motor_id = Some(gripper_motor_id);
        settings.joint_signs = setup.alignment.joint_signs.into();
        settings.joint_offsets_radians = setup.alignment.joint_offsets_radians.into();
        settings.gripper_sign = setup.alignment.gripper_sign;
        settings.gripper_offset_radians = setup.alignment.gripper_offset_radians;
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
        Ok(profile)
    }
}

#[cfg(test)]
mod tests;

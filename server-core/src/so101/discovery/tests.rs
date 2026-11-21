use std::path::{Path, PathBuf};

use serialport::{SerialPortInfo, SerialPortType, UsbPortInfo};
use uuid::Uuid;

use super::*;
use crate::profile::{JointLimitSource, controller_config};
use crate::so101::SO101_JOINT_NAMES;
use crate::so101::calibration::so_follower_calibration_path;
use crate::so101::registry::{CalibrationInstallation, install_calibration};

const ASCENDING_MAPPING: &str = "\n[motor_ids]\nshoulder_pan = 1\nshoulder_lift = 2\nelbow_flex = 3\nwrist_flex = 4\nwrist_roll = 5\ngripper = 6\n";

struct RegistryFixture(PathBuf);

impl RegistryFixture {
    fn new() -> Self {
        let directory = std::env::temp_dir().join(format!("so101-registry-{}", Uuid::new_v4()));
        std::fs::create_dir_all(directory.join("arms")).unwrap();
        Self(directory)
    }

    fn register(&self, serial: &str) -> PathBuf {
        let directory = self.0.join("arms").join(serial);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            directory.join("setup.toml"),
            format!(
                r#"
serial = "{serial}"
[alignment]
joint_offsets_radians = [0.0, 0.0, 0.0, 0.0, -1.5707963267948966]
[gripper]
closed_position_radians = -0.6
open_position_radians = 1.1
{ASCENDING_MAPPING}
"#
            ),
        )
        .unwrap();
        let calibration: serde_json::Map<String, serde_json::Value> = SO101_JOINT_NAMES.into_iter().chain(["gripper"]).zip(1..=6).map(|(joint_name, motor_id)| (
            joint_name.to_owned(),
            serde_json::json!({"id": motor_id, "drive_mode": 0, "homing_offset": motor_id * 10, "range_min": 1023, "range_max": 3071})
        )).collect();
        std::fs::write(
            directory.join("calibration.json"),
            serde_json::to_string(&calibration).unwrap(),
        )
        .unwrap();
        directory
    }
}

impl Drop for RegistryFixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

fn port(name: &str, serial: Option<&str>) -> SerialPortInfo {
    SerialPortInfo {
        port_name: name.to_owned(),
        port_type: SerialPortType::UsbPort(UsbPortInfo {
            vid: 0x1234,
            pid: 0x5678,
            serial_number: serial.map(str::to_owned),
            manufacturer: None,
            product: None,
        }),
    }
}

fn profiles() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("profiles")
}

#[test]
fn native_setup_selects_model_and_preserves_arm_settings() {
    let fixture = RegistryFixture::new();
    let entry = fixture.register("ARM_A");
    std::fs::write(
        entry.join("setup.toml"),
        format!(
            r#"
serial = "ARM_A"
model_variant = "soft-gripper-camera"
[alignment]
joint_signs = [1, -1, 1, 1, 1]
joint_offsets_radians = [0.1, 0.2, 0.3, 0.4, 0.5]
gripper_sign = -1
gripper_offset_radians = 0.6
[gripper]
closed_position_radians = -0.7
open_position_radians = 1.2
{ASCENDING_MAPPING}"#
        ),
    )
    .unwrap();
    let matches = match_controllers(&[port("/dev/a", Some("ARM_A"))], &fixture.0).unwrap();
    let selected = select_controller(&matches, None).unwrap();
    let profile = selected.load_profile(&profiles()).unwrap();
    assert_eq!(profile.guard.heartbeat_timeout_seconds, 3.0);
    let settings = profile.feetech().unwrap();
    assert_eq!(profile.arms, ["arm"]);
    assert_eq!(settings.ports["arm"], "/dev/a");
    assert_eq!(settings.calibration_ids["arm"], "so101_ARM_A");
    assert_eq!(settings.joint_signs[1].as_integer(), -1);
    assert_eq!(settings.joint_offsets_radians[4], 0.5);
    assert_eq!(profile.gripper.unwrap().closed_position_radians, -0.7);
    assert!(
        profile
            .urdf
            .to_string_lossy()
            .contains("soft-gripper-camera")
    );
    let overrides = entry.join("motion.toml");
    std::fs::write(&overrides, "[motion]\nmax_joint_speed = 1.1\n[trajectory]\nacceleration_limit = 2.0\njerk_limit = 8.0\n").unwrap();
    let slower = selected.load_profile(&profiles()).unwrap();
    assert_eq!(slower.motion.max_joint_speed, 1.1);
    assert_eq!(slower.motion.default_joint_speed, 1.1);
    assert_eq!(slower.trajectory.acceleration_limit, 2.0);
    assert_eq!(slower.trajectory.jerk_limit, 8.0);
    assert_eq!(
        slower.tracking.static_margin_radians,
        profile.tracking.static_margin_radians
    );
    for invalid in [
        "[motion]\nmax_joint_speed = 0.0\n",
        "[motion]\nmax_joint_speed = 1.0\ndefault_joint_speed = 1.5\n",
        "[trajectory]\njerk_limit = nan\n",
    ] {
        std::fs::write(&overrides, invalid).unwrap();
        assert!(selected.load_profile(&profiles()).is_err(), "{invalid}");
    }
    std::fs::remove_file(overrides).unwrap();
    let setup_path = entry.join("setup.toml");
    let setup = std::fs::read_to_string(&setup_path)
        .unwrap()
        .replace("ARM_A", "WRONG");
    std::fs::write(&setup_path, setup).unwrap();
    assert!(selected.load_profile(&profiles()).is_err());
    std::fs::write(
        &setup_path,
        format!("serial = \"ARM_A\"\n{ASCENDING_MAPPING}"),
    )
    .unwrap();
    std::fs::remove_file(&setup_path).unwrap();
    assert!(
        selected
            .load_profile(&profiles())
            .unwrap_err()
            .to_string()
            .contains("run SO-101 setup")
    );
}

#[test]
fn native_setup_inherits_unset_settings_from_the_profile() {
    let fixture = RegistryFixture::new();
    let entry = fixture.register("ARM_A");
    std::fs::write(entry.join("setup.toml"), "serial = \"ARM_A\"\n").unwrap();
    let matches = match_controllers(&[port("/dev/a", Some("ARM_A"))], &fixture.0).unwrap();
    let profile = select_controller(&matches, None)
        .unwrap()
        .load_profile(&profiles())
        .unwrap();
    let settings = profile.feetech().unwrap();
    assert_eq!(settings.joint_offsets_radians, [0.0; 5]);
    assert_eq!(settings.motor_ids, [1, 2, 3, 4, 5]);
    assert_eq!(profile.gripper.unwrap().open_position_radians, 1.5);
}

#[test]
fn registry_selection_produces_the_arm_config_without_manual_port_or_calibration_copying() {
    let fixture = RegistryFixture::new();
    let entry = fixture.register("ARM_A");
    let matches = match_controllers(
        &[
            port("/dev/ttyUSB7", Some("ARM_A")),
            port("/dev/ttyUSB8", Some("OTHER")),
        ],
        &fixture.0,
    )
    .unwrap();
    let selected = select_controller(&matches, None).unwrap();
    let profile = selected.load_profile(&profiles()).unwrap();
    assert_eq!(profile.name, "so101-ARM_A");
    let settings = profile.feetech().unwrap();
    let calibration_root = fixture.0.join("calibration");
    assert!(matches!(
        install_calibration(
            &entry.join("calibration.json"),
            settings,
            "arm",
            &calibration_root
        )
        .unwrap(),
        CalibrationInstallation::Installed(_)
    ));
    let config = controller_config(
        &profile,
        "arm",
        JointLimitSource::Calibration,
        &calibration_root,
    )
    .unwrap();
    let hardware = config.feetech.as_ref().unwrap();
    assert_eq!(hardware.port, "/dev/ttyUSB7");
    assert_eq!(
        hardware.joints[4].offset_radians,
        -std::f64::consts::FRAC_PI_2
    );
    assert_eq!(hardware.joints[1].calibration.homing_offset, 20);
    assert_eq!(config.gripper.as_ref().unwrap().closed_radians, -0.6);
    assert!(config.lower_limits[0] < -1.57 && config.upper_limits[0] > 1.57);
}

#[test]
fn identical_calibration_is_reused_and_a_local_change_is_preserved() {
    let fixture = RegistryFixture::new();
    let entry = fixture.register("ARM_A");
    let matches = match_controllers(&[port("/dev/a", Some("ARM_A"))], &fixture.0).unwrap();
    let selected = select_controller(&matches, None).unwrap();
    let profile = selected.load_profile(&profiles()).unwrap();
    let settings = profile.feetech().unwrap();
    let calibration_root = fixture.0.join("calibration");
    let source = entry.join("calibration.json");
    install_calibration(&source, settings, "arm", &calibration_root).unwrap();
    assert!(matches!(
        install_calibration(&source, settings, "arm", &calibration_root).unwrap(),
        CalibrationInstallation::Unchanged(_)
    ));
    let destination = so_follower_calibration_path(&calibration_root, "so101_ARM_A");
    let mut changed: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&destination).unwrap()).unwrap();
    changed["shoulder_pan"]["homing_offset"] = serde_json::json!(300);
    let changed = changed.to_string();
    std::fs::write(&destination, &changed).unwrap();
    assert_eq!(
        install_calibration(&source, settings, "arm", &calibration_root).unwrap(),
        CalibrationInstallation::Conflict(destination.clone())
    );
    assert_eq!(std::fs::read_to_string(&destination).unwrap(), changed);
}

#[test]
fn invalid_registry_calibration_never_installs_a_partial_file() {
    let fixture = RegistryFixture::new();
    let entry = fixture.register("ARM_A");
    let matches = match_controllers(&[port("/dev/a", Some("ARM_A"))], &fixture.0).unwrap();
    let profile = select_controller(&matches, None)
        .unwrap()
        .load_profile(&profiles())
        .unwrap();
    let source = entry.join("calibration.json");
    let calibration_root = fixture.0.join("calibration");
    let valid = std::fs::read_to_string(&source).unwrap();
    for invalid in [
        valid.replace("3071", "5000"),
        valid.replace("\"id\":2", "\"id\":1"),
        "{}".to_owned(),
    ] {
        std::fs::write(&source, invalid).unwrap();
        assert!(
            install_calibration(
                &source,
                profile.feetech().unwrap(),
                "arm",
                &calibration_root
            )
            .is_err()
        );
        assert!(!calibration_root.exists());
    }
}

#[test]
fn native_setup_routes_scrambled_calibration_to_the_physical_joints() {
    let fixture = RegistryFixture::new();
    let entry = fixture.register("ARM_A");
    std::fs::write(
        entry.join("setup.toml"),
        "serial = \"ARM_A\"\n[motor_ids]\nshoulder_pan = 6\nshoulder_lift = 3\nelbow_flex = 1\nwrist_flex = 5\nwrist_roll = 2\ngripper = 4\n",
    )
    .unwrap();
    let matches = match_controllers(&[port("/dev/a", Some("ARM_A"))], &fixture.0).unwrap();
    let selected = select_controller(&matches, None).unwrap();
    assert!(
        selected
            .load_profile(&profiles())
            .unwrap_err()
            .to_string()
            .contains("motor ID mapping")
    );
    let calibration_path = entry.join("calibration.json");
    let mut calibration: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&calibration_path).unwrap()).unwrap();
    for (joint_name, id) in SO101_JOINT_NAMES
        .into_iter()
        .chain(["gripper"])
        .zip([6, 3, 1, 5, 2, 4])
    {
        calibration[joint_name]["id"] = serde_json::json!(id);
    }
    std::fs::write(&calibration_path, calibration.to_string()).unwrap();
    let profile = selected.load_profile(&profiles()).unwrap();
    let settings = profile.feetech().unwrap();
    let calibration_root = fixture.0.join("calibration");
    install_calibration(&calibration_path, settings, "arm", &calibration_root).unwrap();
    let config = controller_config(
        &profile,
        "arm",
        JointLimitSource::Calibration,
        &calibration_root,
    )
    .unwrap();
    let hardware = config.feetech.as_ref().unwrap();
    assert_eq!(
        hardware
            .joints
            .iter()
            .map(|joint| joint.id)
            .collect::<Vec<_>>(),
        [6, 3, 1, 5, 2]
    );
    assert_eq!(hardware.joints[0].calibration.homing_offset, 10);
    assert_eq!(hardware.gripper.as_ref().unwrap().motor.id, 4);
    let proportional_gains = hardware
        .joints
        .iter()
        .map(|joint| joint.expected_gains.p)
        .collect::<Vec<_>>();
    assert_eq!(proportional_gains, [16, 32, 32, 16, 16]);
}

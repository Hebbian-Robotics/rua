use std::path::{Path, PathBuf};

use serialport::UsbPortInfo;
use uuid::Uuid;

use super::*;
use crate::profile::{JointLimitSource, executor_config};
use crate::so101::calibration::so_follower_calibration_path;
use crate::so101::registry::{CalibrationInstallation, install_calibration};

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
            directory.join("profile.toml"),
            format!(
                r#"
extends = "so101-manual"
name = "so101-{serial}"
[feetech]
ports = {{ follower = "__LOCAL_PORT__" }}
calibration_ids = {{ follower = "so101_{serial}" }}
joint_offsets_radians = [0.0, 0.0, 0.0, 0.0, -1.5707963267948966]
[gripper]
closed_position_radians = -0.6
open_position_radians = 1.1
"#
            ),
        )
        .unwrap();
        let calibration: serde_json::Map<String, serde_json::Value> = (1..=6).map(|motor_id| (
            format!("motor_{motor_id}"),
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
            "follower",
            &calibration_root
        )
        .unwrap(),
        CalibrationInstallation::Installed(_)
    ));
    let config = executor_config(
        &profile,
        "follower",
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
fn selecting_among_multiple_arms_requires_a_serial_and_rejects_duplicate_serials() {
    let fixture = RegistryFixture::new();
    fixture.register("ARM_A");
    fixture.register("ARM_B");
    let ports = [
        port("/dev/ttyUSB0", Some("ARM_A")),
        port("/dev/ttyUSB1", Some("ARM_B")),
    ];
    let matches = match_controllers(&ports, &fixture.0).unwrap();
    assert!(
        select_controller(&matches, None)
            .unwrap_err()
            .to_string()
            .contains("Multiple registered")
    );
    let requested = ControllerSerial::parse("ARM_B").unwrap();
    assert_eq!(
        select_controller(&matches, Some(&requested))
            .unwrap()
            .controller
            .port,
        "/dev/ttyUSB1"
    );
    let duplicates = [
        port("/dev/ttyUSB0", Some("ARM_A")),
        port("/dev/ttyUSB2", Some("ARM_A")),
    ];
    let matches = match_controllers(&duplicates, &fixture.0).unwrap();
    assert!(select_controller(&matches, None).is_err());
    assert!(select_controller(&matches, Some(&ControllerSerial::parse("ARM_A").unwrap())).is_err());
}

#[test]
fn unknown_missing_and_unsafe_serials_never_select_a_configuration() {
    let fixture = RegistryFixture::new();
    let matches = match_controllers(
        &[
            port("/dev/a", Some("UNKNOWN")),
            port("/dev/b", None),
            port("/dev/c", Some("../elsewhere")),
        ],
        &fixture.0,
    )
    .unwrap();
    assert!(
        matches
            .iter()
            .any(|candidate| matches!(candidate, ControllerMatch::Unregistered { .. }))
    );
    assert!(
        matches
            .iter()
            .any(|candidate| matches!(candidate, ControllerMatch::MissingSerial { .. }))
    );
    assert!(
        matches
            .iter()
            .any(|candidate| matches!(candidate, ControllerMatch::InvalidSerial { .. }))
    );
    assert!(select_controller(&matches, None).is_err());
    assert!(ControllerSerial::parse("").is_err());
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
    install_calibration(&source, settings, "follower", &calibration_root).unwrap();
    assert!(matches!(
        install_calibration(&source, settings, "follower", &calibration_root).unwrap(),
        CalibrationInstallation::Unchanged(_)
    ));
    let destination = so_follower_calibration_path(&calibration_root, "so101_ARM_A");
    let mut changed: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&destination).unwrap()).unwrap();
    changed["motor_1"]["homing_offset"] = serde_json::json!(300);
    let changed = changed.to_string();
    std::fs::write(&destination, &changed).unwrap();
    assert_eq!(
        install_calibration(&source, settings, "follower", &calibration_root).unwrap(),
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
                "follower",
                &calibration_root
            )
            .is_err()
        );
        assert!(!calibration_root.exists());
    }
}

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;

fn bundled_profiles() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("profiles")
}

#[test]
fn an_external_profile_inherits_bundled_defaults_and_resolves_its_own_assets() {
    let directory = scratch_directory();
    let path = directory.join("my-arm.toml");
    std::fs::write(
        &path,
        r#"extends = "so101-sim"
name = "my-arm"
urdf = "custom.urdf"
"#,
    )
    .unwrap();
    let profile = load_robot_profile(&bundled_profiles(), path.to_str().unwrap()).unwrap();
    assert_eq!(profile.name, "my-arm");
    assert_eq!(profile.urdf, directory.join("custom.urdf"));
    assert!(matches!(profile.driver, Driver::Simulator));
}

#[test]
fn a_profile_given_as_json_matches_its_toml() {
    let from_toml = load_robot_profile(&bundled_profiles(), "so101-bimanual").unwrap();
    let tables = profile_tables(&bundled_profiles(), "so101-bimanual", 0).unwrap();
    let text = serde_json::to_string(&tables).unwrap();
    let root = bundled_profiles().parent().unwrap().to_owned();
    let from_json = parse_robot_profile_json(&text, &root).unwrap();
    assert_eq!(from_json.arms, from_toml.arms);
    assert_eq!(from_json.urdf, from_toml.urdf);
    assert!(parse_robot_profile_json("{\"name\": \"x\"}", &root).is_err());
}

const BUNDLED_PROFILES: [&str; 7] = [
    "so101",
    "so101-bimanual",
    "so101-manual",
    "so101-policy",
    "so101-sim",
    "so101-bimanual-sim",
    "so101-sim-policy",
];

fn scratch_directory() -> PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let directory = std::env::temp_dir().join(format!(
        "hebbian-profile-test-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&directory).unwrap();
    directory
}

/// A profile directory holding the bundled SO-101 profiles and one more.
fn profiles_with(name: &str, text: &str) -> PathBuf {
    let directory = scratch_directory().join("profiles");
    std::fs::create_dir_all(&directory).unwrap();
    for base in ["so101-description", "so101"] {
        std::fs::copy(
            bundled_profiles().join(format!("{base}.toml")),
            directory.join(format!("{base}.toml")),
        )
        .unwrap();
    }
    std::fs::write(directory.join(format!("{name}.toml")), text).unwrap();
    directory
}

fn write_calibration(root: &Path, calibration_id: &str) {
    let directory = root.join("robots").join("so_follower");
    std::fs::create_dir_all(&directory).unwrap();
    let entries: serde_json::Map<String, serde_json::Value> = (1..=6)
        .map(|id| {
            (
                format!("motor_{id}"),
                serde_json::json!({
                    "id": id, "drive_mode": 0, "homing_offset": 10 * id,
                    "range_min": 1023, "range_max": 3071,
                }),
            )
        })
        .collect();
    std::fs::write(
        directory.join(format!("{calibration_id}.json")),
        serde_json::Value::Object(entries).to_string(),
    )
    .unwrap();
}

#[test]
fn every_bundled_profile_loads() {
    for name in BUNDLED_PROFILES {
        load_robot_profile(&bundled_profiles(), name)
            .unwrap_or_else(|error| panic!("{name}: {error:#}"));
    }
}

#[test]
fn an_extending_profile_replaces_values_inside_merged_tables() {
    let profile = load_robot_profile(&bundled_profiles(), "so101-bimanual").unwrap();
    assert_eq!(profile.arms, ["left", "right"]);
    let ports: Vec<&String> = profile.feetech().unwrap().ports.keys().collect();
    assert_eq!(ports, ["left", "right"]);
    // The base's tracking table survives the merge.
    assert!((profile.tracking.static_margin_radians - 0.1).abs() < 1e-12);
    assert!(profile.urdf.is_absolute());
}

#[test]
fn the_policy_controller_runs_at_its_own_rate_and_speed() {
    let profile = load_robot_profile(&bundled_profiles(), "so101-sim-policy").unwrap();
    let policy = profile.policy.as_ref().unwrap();
    assert!((profile.control.frequency_hertz - POLICY_CONTROL_HERTZ).abs() < 1e-12);
    assert!((profile.motion.max_joint_speed - policy.max_joint_speed).abs() < 1e-12);
}

#[test]
fn a_feetech_arm_takes_its_limits_and_motors_from_its_calibration() {
    let profile = load_robot_profile(&bundled_profiles(), "so101").unwrap();
    let calibration_root = scratch_directory();
    write_calibration(&calibration_root, "follower");
    let config = executor_config(
        &profile,
        "follower",
        JointLimitSource::Calibration,
        &calibration_root,
    )
    .unwrap();
    // Steps 1023..3071 around the 2047 homing pose are -1/4 .. +1/4 turn.
    let quarter_turn = std::f64::consts::FRAC_PI_2;
    assert!((config.lower_limits[0] + quarter_turn).abs() < 1e-3);
    assert!((config.upper_limits[0] - quarter_turn).abs() < 1e-3);
    let feetech = config.feetech.as_ref().unwrap();
    assert_eq!(feetech.joints[2].calibration.homing_offset, 30);
    assert_eq!(feetech.gripper.as_ref().unwrap().motor.id, 6);
    assert_eq!(config.driven_joints.last().unwrap(), "gripper");
    assert!(config.guard.is_some() && config.enable_checks.is_some());
}

#[test]
fn a_feetech_arm_without_its_calibration_is_refused() {
    let profile = load_robot_profile(&bundled_profiles(), "so101").unwrap();
    let error = executor_config(
        &profile,
        "follower",
        JointLimitSource::Urdf,
        &scratch_directory(),
    )
    .unwrap_err();
    assert!(format!("{error:#}").contains("calibration"), "{error:#}");
}

#[test]
fn an_unknown_arm_is_refused() {
    let profile = load_robot_profile(&bundled_profiles(), "so101").unwrap();
    assert!(
        executor_config(
            &profile,
            "nobody",
            JointLimitSource::Urdf,
            &scratch_directory()
        )
        .is_err()
    );
}

#[test]
fn a_rest_reference_checks_each_joint_without_assuming_support() {
    let profile = load_robot_profile(&bundled_profiles(), "so101").unwrap();
    let reference = profile.rest_reference.unwrap();
    let mut positions = reference.joint_positions_radians.clone();
    assert!(reference.matches(&positions));
    assert!(!reference.matches(&positions[..4]));
    positions[1] += reference.joint_tolerances_radians[1] + 0.01;
    assert!(!reference.matches(&positions));
}

#[test]
fn malformed_profiles_are_refused() {
    let cases = [
        ("unknown key", "extends = \"so101\"\nspeed_boost = 2\n"),
        (
            "bad rest reference",
            "extends = \"so101\"\n[rest_reference]\njoint_positions_radians = [0.0]\n",
        ),
        (
            "sign",
            "extends = \"so101\"\n[feetech]\njoint_signs = [1, 1, 2, 1, 1]\n",
        ),
        (
            "missing port",
            "extends = \"so101\"\narms = [\"follower\", \"spare\"]\n",
        ),
    ];
    for (case, text) in cases {
        let directory = profiles_with("broken", text);
        assert!(
            load_robot_profile(&directory, "broken").is_err(),
            "{case} was accepted"
        );
    }
}

#[test]
fn an_external_simulator_needs_no_engine_or_hardware_settings() {
    let profile = load_robot_profile(&bundled_profiles(), "so101-socket").unwrap();
    assert!(matches!(profile.driver, Driver::Simulator));
    assert!(profile.feetech().is_none());
    assert!(
        executor_config(
            &profile,
            "follower",
            JointLimitSource::Urdf,
            &scratch_directory()
        )
        .is_ok()
    );
}

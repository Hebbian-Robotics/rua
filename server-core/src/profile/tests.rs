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
allowed_collision_pairs = [["first_link", "second_link"]]
"#,
    )
    .unwrap();
    let profile = load_robot_profile(&bundled_profiles(), path.to_str().unwrap()).unwrap();
    assert_eq!(profile.name, "my-arm");
    assert_eq!(profile.urdf, directory.join("custom.urdf"));
    assert!(matches!(profile.driver, Driver::Simulator));
    assert_eq!(
        profile.allowed_collision_pairs,
        [["first_link", "second_link"]]
    );
}

#[test]
fn profile_arm_names_must_be_unique_socket_names() {
    for (arms, expected_error) in [
        (r#"["arm", "arm"]"#, "Duplicate arm name"),
        (r#"["arm/other"]"#, "Invalid arm name"),
    ] {
        let path = scratch_directory().join("invalid-arms.toml");
        std::fs::write(&path, format!("extends = \"so101-sim\"\narms = {arms}\n")).unwrap();
        let error = load_robot_profile(&bundled_profiles(), path.to_str().unwrap()).unwrap_err();
        assert!(format!("{error:#}").contains(expected_error));
    }
}

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

fn bimanual_hardware_profile() -> PathBuf {
    let path = scratch_directory().join("bimanual.toml");
    std::fs::write(
        &path,
        r#"extends = ["so101", "so101-bimanual-layout"]
name = "bimanual"
[feetech]
ports = { left = "/dev/left", right = "/dev/right" }
calibration_ids = { left = "left", right = "right" }
"#,
    )
    .unwrap();
    path
}

fn profiles_with(name: &str, text: &str) -> PathBuf {
    let directory = scratch_directory().join("profiles");
    std::fs::create_dir_all(&directory).unwrap();
    for base in ["so101-description", "so101-common", "so101"] {
        std::fs::copy(
            bundled_profiles().join(format!("{base}.toml")),
            directory.join(format!("{base}.toml")),
        )
        .unwrap();
    }
    let manifest_path = Path::new("robots/so101/robot-assemblies.toml");
    let manifest = directory.join("..").join(manifest_path);
    std::fs::create_dir_all(manifest.parent().unwrap()).unwrap();
    std::fs::copy(bundled_profiles().join("..").join(manifest_path), manifest).unwrap();
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
fn bundled_profiles_offer_only_hardware_and_simulator_presets() {
    let directory = bundled_profiles();
    let mut runnable = Vec::new();
    for entry in std::fs::read_dir(&directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_file()
            && path
                .extension()
                .is_some_and(|extension| extension == "toml")
        {
            let name = path.file_stem().unwrap().to_str().unwrap();
            let tables = profile_tables(&directory, name, 0).unwrap();
            if tables.contains_key("name") {
                load_robot_profile(&directory, name)
                    .unwrap_or_else(|error| panic!("{name}: {error:#}"));
                runnable.push(name.to_owned());
            }
        }
    }
    runnable.sort();
    assert_eq!(
        runnable,
        ["so101", "so101-sim"],
        "Put scenario combinations in their owning example or local configuration"
    );
}

#[test]
fn an_extending_profile_replaces_values_inside_merged_tables() {
    let path = bimanual_hardware_profile();
    let profile = load_robot_profile(&bundled_profiles(), path.to_str().unwrap()).unwrap();
    assert_eq!(profile.arms, ["left", "right"]);
    let ports: Vec<&String> = profile.feetech().unwrap().ports.keys().collect();
    assert_eq!(ports, ["left", "right"]);
    assert!((profile.tracking.static_margin_radians - 0.1).abs() < 1e-12);
    assert!(profile.urdf.is_absolute());
}

#[test]
fn the_policy_controller_runs_at_its_own_rate_and_speed() {
    let profile = load_robot_profile_with_fragments(
        &bundled_profiles(),
        "so101-sim",
        &[
            "so101-soft-gripper-camera-assembly",
            "so101-policy-settings",
        ],
    )
    .unwrap();
    let policy = profile.policy.as_ref().unwrap();
    assert!((profile.control.frequency_hertz - POLICY_CONTROL_HERTZ).abs() < 1e-12);
    assert!((profile.motion.max_joint_speed - policy.max_joint_speed).abs() < 1e-12);
}

#[test]
fn a_feetech_arm_takes_its_limits_and_motors_from_its_calibration() {
    let profile = load_robot_profile(&bundled_profiles(), "so101").unwrap();
    let calibration_root = scratch_directory();
    write_calibration(&calibration_root, "arm");
    let config = controller_config(
        &profile,
        "arm",
        JointLimitSource::Calibration,
        &calibration_root,
    )
    .unwrap();
    // INVARIANT: Steps 1023..3071 around the 2047 homing pose are -1/4 .. +1/4 turn.
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
    let error = controller_config(
        &profile,
        "arm",
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
        controller_config(
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
            "extends = \"so101\"\narms = [\"arm\", \"spare\"]\n",
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
    let profile = load_robot_profile(&bundled_profiles(), "so101-sim").unwrap();
    assert!(matches!(profile.driver, Driver::Simulator));
    assert!(profile.feetech().is_none());
    assert!(
        controller_config(
            &profile,
            "arm",
            JointLimitSource::Urdf,
            &scratch_directory()
        )
        .is_ok()
    );
}

#[test]
fn simulator_config_uses_resolved_rest_pose_effort_units_and_gripper_force() {
    let directory = scratch_directory();
    let profile_path = directory.join("custom.toml");
    std::fs::write(
        &profile_path,
        r#"extends = "so101-sim"
[rest_reference]
joint_positions_radians = [0.0, -0.8, 1.0, 0.5, 0.0]
[effort]
unit = "newton_metres"
joint_effort_limits = [1.0, 1.0, 1.0, 1.0, 1.0]
[gripper]
force_fraction = 0.2
"#,
    )
    .unwrap();
    let profile = load_robot_profile(&bundled_profiles(), profile_path.to_str().unwrap()).unwrap();
    let config = controller_config(&profile, "arm", JointLimitSource::Urdf, &directory).unwrap();
    let resolved = serde_json::to_value(config).unwrap();
    assert_eq!(
        resolved["simulation_model"],
        serde_json::json!({
            "effort_unit": "newton_metres",
            "initial_joint_positions_radians": [0.0, -0.8, 1.0, 0.5, 0.0],
            "gripper_force_fraction": 0.2,
        })
    );
}

#[test]
fn fixture_profiles_share_the_hardware_control_defaults() {
    let directory = bundled_profiles();
    let hardware = load_robot_profile(&directory, "so101").unwrap();
    let fixtures = directory
        .parent()
        .unwrap()
        .join("examples/mujoco_sim/profiles");
    for name in ["single", "bimanual", "policy"] {
        let path = fixtures.join(format!("{name}.toml"));
        let fixture = load_robot_profile(&directory, path.to_str().unwrap()).unwrap();
        assert_eq!(fixture.control, hardware.control, "{name}");
        assert_eq!(fixture.tracking, hardware.tracking, "{name}");
        assert_eq!(fixture.trajectory, hardware.trajectory, "{name}");
        assert_eq!(fixture.motion, hardware.motion, "{name}");
        assert_eq!(fixture.gripper, hardware.gripper, "{name}");
        assert_eq!(fixture.effort, hardware.effort, "{name}");
        assert_eq!(fixture.surface_guard, hardware.surface_guard, "{name}");
        assert_eq!(
            fixture.joint_limit_recovery_tolerance_radians,
            hardware.joint_limit_recovery_tolerance_radians,
            "{name}"
        );
    }
}

#[test]
fn invalid_inheritance_and_base_poses_are_rejected() {
    let directory = scratch_directory();
    let profile_path = directory.join("invalid.toml");
    let profile_name = profile_path.to_str().unwrap();
    for (text, reason) in [
        ("extends = [1]".to_owned(), "extends names a non-string"),
        (format!("extends = {profile_name:?}"), "inheritance limit"),
        (
            "extends = \"so101-sim\"\n[base_poses.nobody]\nposition_meters = [0, 0, 0]\n"
                .to_owned(),
            "unknown arm",
        ),
        (
            "extends = \"so101-sim\"\n[base_poses.arm]\nposition_meters = [0, 0, nan]\n".to_owned(),
            "finite coordinates and yaw",
        ),
    ] {
        std::fs::write(&profile_path, text).unwrap();
        let error = load_robot_profile(&bundled_profiles(), profile_name).unwrap_err();
        assert!(format!("{error:#}").contains(reason), "{error:#}");
    }
}

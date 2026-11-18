use std::path::Path;

use anyhow::{Context, bail, ensure};
use sha2::{Digest, Sha256};

use super::{PolicySettings, RobotProfile};
use crate::config::{
    BasePose, EnableChecks, ExecutorConfig, ExecutorConfigInput, GripperLimits, GuardConfig,
    MotionLimits, PolicyConfig, SelfCollisionConfig,
};
use crate::so101::calibration::calibrated_joint_limits;
use crate::so101::executor_config::feetech_bus;
use hebbian_contracts::SurfaceGuardConfig;

const STREAM_GOAL_TIMEOUT_SECONDS: f64 = 0.5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JointLimitSource {
    Calibration,
    Urdf,
}

pub fn urdf_joint_limits(urdf: &Path, joint_names: &[String]) -> anyhow::Result<Vec<(f64, f64)>> {
    let robot = urdf_rs::read_file(urdf)
        .with_context(|| format!("Cannot read the URDF {}", urdf.display()))?;
    joint_names
        .iter()
        .map(|name| {
            let joint = robot
                .joints
                .iter()
                .find(|joint| &joint.name == name)
                .with_context(|| format!("{} has no joint {name}", urdf.display()))?;
            Ok((joint.limit.lower, joint.limit.upper))
        })
        .collect()
}

fn sha256_hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    Sha256::digest(bytes)
        .iter()
        .fold(String::with_capacity(64), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        })
}

// WHY: Blank mesh paths before hashing so relocated copies and trailing whitespace do not invalidate the tool fixture.
fn urdf_fingerprint(urdf: &Path) -> anyhow::Result<String> {
    const MARKER: &str = "filename=\"";
    let text = std::fs::read_to_string(urdf)
        .with_context(|| format!("Cannot read the URDF {}", urdf.display()))?;
    let mut blanked = String::with_capacity(text.len());
    let mut rest = text.as_str();
    while let Some(start) = rest.find(MARKER) {
        let after_marker = &rest[start + MARKER.len()..];
        let Some(end) = after_marker.find('"') else {
            break;
        };
        blanked.push_str(&rest[..start]);
        blanked.push_str("filename=\"\"");
        rest = &after_marker[end + 1..];
    }
    blanked.push_str(rest);
    let normalized = blanked
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n");
    Ok(sha256_hex(normalized.as_bytes()))
}

// SAFETY: Refuse a URDF other than the one the tool-frame fixture was fitted to: the
// fixture records its URDF's hash, and that file ships beside it.
fn require_urdf_behind_tool_frame_fixture(urdf: &Path, fixture: &Path) -> anyhow::Result<()> {
    let fixture_text = std::fs::read_to_string(fixture)
        .with_context(|| format!("Cannot read {}", fixture.display()))?;
    let fixture_json: serde_json::Value = serde_json::from_str(&fixture_text)
        .with_context(|| format!("Cannot parse {}", fixture.display()))?;
    let fitted_hash = fixture_json["station_urdf_sha256"]
        .as_str()
        .with_context(|| format!("{} has no station_urdf_sha256", fixture.display()))?;
    let directory = fixture.parent().unwrap_or(Path::new("."));
    for entry in std::fs::read_dir(directory)? {
        let candidate = entry?.path();
        if candidate
            .extension()
            .is_none_or(|extension| extension != "urdf")
        {
            continue;
        }
        if sha256_hex(&std::fs::read(&candidate)?) == fitted_hash {
            ensure!(
                urdf_fingerprint(&candidate)? == urdf_fingerprint(urdf)?,
                "{} differs from {}, the URDF that {} was fitted to; refit the fixture or use that model",
                urdf.display(),
                candidate.display(),
                fixture.display()
            );
            return Ok(());
        }
    }
    bail!(
        "No URDF with sha256 {} (the one {} was fitted to) beside it; cannot confirm {} matches it",
        &fitted_hash[..fitted_hash.len().min(12)],
        fixture.display(),
        urdf.display()
    )
}

fn policy_config(
    profile: &RobotProfile,
    policy: &PolicySettings,
    arm: &str,
) -> anyhow::Result<PolicyConfig> {
    let onnx = policy
        .onnx
        .as_ref()
        .filter(|onnx| onnx.is_file())
        .with_context(|| {
            format!(
                "Profile {} selects the policy controller but its exported model is missing: \
             provide a compatible ONNX model and set [policy] onnx",
                profile.name
            )
        })?;
    require_urdf_behind_tool_frame_fixture(&profile.urdf, &policy.tool_frame_fixture)?;
    Ok(PolicyConfig {
        reach_deadline_seconds: policy.reach_deadline_seconds,
        settle_dwell_seconds: policy.settle_dwell_seconds,
        settled_velocity_radians_per_second: policy.settled_velocity_radians_per_second,
        preview_lag_seconds: policy.preview_lag_seconds,
        onnx: std::fs::canonicalize(onnx)?.display().to_string(),
        tool_frame_fixture: std::fs::canonicalize(&policy.tool_frame_fixture)?
            .display()
            .to_string(),
        checkpoint_sha256: policy.checkpoint_sha256.clone(),
        max_target_lead_radians: policy.max_target_lead_radians,
        tool_link: profile.arm_tool_frame(arm),
        position_tolerance_meters: policy.position_tolerance_meters,
        orientation_tolerance_radians: policy.orientation_tolerance_radians,
    })
}

pub fn executor_config(
    profile: &RobotProfile,
    arm: &str,
    joint_limits: JointLimitSource,
    calibration_root: &Path,
) -> anyhow::Result<ExecutorConfig> {
    ExecutorConfig::try_from(executor_config_input(
        profile,
        arm,
        joint_limits,
        calibration_root,
    )?)
    .with_context(|| format!("The config for arm {arm} of {}", profile.name))
}

pub fn executor_config_input(
    profile: &RobotProfile,
    arm: &str,
    joint_limits: JointLimitSource,
    calibration_root: &Path,
) -> anyhow::Result<ExecutorConfigInput> {
    ensure!(
        profile.arms.iter().any(|known| known == arm),
        "Profile {} has no arm {arm}",
        profile.name
    );
    let limits = match (joint_limits, profile.feetech()) {
        (JointLimitSource::Calibration, Some(settings)) => {
            calibrated_joint_limits(settings, arm, calibration_root)?
        }
        _ => urdf_joint_limits(&profile.urdf, &profile.arm_joint_names(arm))?,
    };
    let feetech = profile
        .feetech()
        .map(|settings| feetech_bus(profile.gripper.as_ref(), settings, arm, calibration_root))
        .transpose()?;
    let has_feetech = feetech.is_some();
    let config = ExecutorConfigInput {
        urdf: std::fs::canonicalize(&profile.urdf)
            .with_context(|| format!("Cannot find the URDF {}", profile.urdf.display()))?
            .display()
            .to_string(),
        driven_joints: profile
            .arm_joint_names(arm)
            .into_iter()
            .chain(profile.gripper.as_ref().map(|_| "gripper".to_owned()))
            .collect(),
        surface_guard: profile.surface_guard.enabled.then_some(SurfaceGuardConfig {
            surface_height_meters: profile.surface_guard.surface_height_meters,
            leeway_meters: profile.surface_guard.leeway_meters,
        }),
        self_collision: profile.self_collision_penetration_tolerance_meters.map(
            |penetration_tolerance_meters| SelfCollisionConfig {
                penetration_tolerance_meters,
                allowed_collision_pairs: profile.allowed_collision_pairs.clone(),
            },
        ),
        feetech,
        sim_plant: None,
        enable_checks: has_feetech.then_some(EnableChecks {
            preflight_drift_limit_radians: profile.enable.preflight_drift_limit_radians,
            enable_drift_limit_radians: profile.enable.enable_drift_limit_radians,
        }),
        guard: has_feetech.then_some(GuardConfig {
            heartbeat_timeout_seconds: profile.guard.heartbeat_timeout_seconds,
            poll_seconds: profile.guard.poll_seconds,
            hold_limit_seconds: profile.guard.hold_limit_seconds,
        }),
        tool_link: Some(profile.arm_tool_frame(arm)),
        base_pose: profile.base_poses.get(arm).map(|base| BasePose {
            position_meters: base.position_meters,
            yaw_radians: base.yaw_radians,
        }),
        policy: profile
            .policy
            .as_ref()
            .map(|policy| policy_config(profile, policy, arm))
            .transpose()?,
        gripper: profile.gripper.as_ref().map(|gripper| GripperLimits {
            closed_radians: gripper.closed_position_radians,
            open_radians: gripper.open_position_radians,
            position_tolerance_radians: gripper.position_tolerance_radians,
            contact_dwell_seconds: gripper.contact_dwell_seconds,
        }),
        trace_directory: crate::config::default_trace_directory(),
        motion: MotionLimits {
            acceleration_limit: profile.trajectory.acceleration_limit,
            jerk_limit: profile.trajectory.jerk_limit,
            settled_error_radians: profile.tracking.settled_error_radians,
            settle_dwell_seconds: profile.tracking.settle_dwell_seconds,
            settle_timeout_seconds: profile.tracking.settle_timeout_seconds,
        },
        joint_count: profile.joint_count(),
        frequency_hertz: profile.control.frequency_hertz,
        loop_deadline_seconds: profile.control.loop_deadline_seconds,
        lower_limits: limits.iter().map(|(lower, _)| *lower).collect(),
        upper_limits: limits.iter().map(|(_, upper)| *upper).collect(),
        joint_effort_limits: profile.effort.joint_effort_limits.clone(),
        tracking_following_lag_seconds: profile.tracking.following_lag_seconds,
        tracking_static_margin_radians: profile.tracking.static_margin_radians,
        max_joint_speed: profile.motion.max_joint_speed,
        default_joint_speed: profile.motion.default_joint_speed,
        stream_goal_timeout_seconds: STREAM_GOAL_TIMEOUT_SECONDS,
        joint_limit_recovery_tolerance_radians: profile.joint_limit_recovery_tolerance_radians,
        jaw_closing_axis: profile.jaw_closing_axis,
        station: Some(crate::config::StationDescription {
            robot: profile.name.clone(),
            description: profile.description.clone(),
            joint_names: profile.joint_names.clone(),
        }),
    };
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tool_frame_fixture_accepts_its_urdf_with_moved_meshes_and_refuses_another() {
        let scratch =
            std::env::temp_dir().join(format!("hebbian-fixture-test-{}", std::process::id()));
        let fixture_directory = scratch.join("fixture");
        let models = scratch.join("models");
        std::fs::create_dir_all(&fixture_directory).unwrap();
        std::fs::create_dir_all(&models).unwrap();
        let fitted = r#"<robot name="a"><mesh filename="/original/base.stl"/></robot>"#;
        std::fs::write(fixture_directory.join("station-model.urdf"), fitted).unwrap();
        let fixture = fixture_directory.join("fixture.json");
        let fixture_json =
            serde_json::json!({ "station_urdf_sha256": sha256_hex(fitted.as_bytes()) });
        std::fs::write(&fixture, fixture_json.to_string()).unwrap();

        let moved_meshes = models.join("moved.urdf");
        std::fs::write(&moved_meshes, fitted.replace("/original/", "assets/")).unwrap();
        require_urdf_behind_tool_frame_fixture(&moved_meshes, &fixture).unwrap();

        let trailing_whitespace = models.join("trailing-whitespace.urdf");
        std::fs::write(&trailing_whitespace, format!("{fitted}  \n")).unwrap();
        require_urdf_behind_tool_frame_fixture(&trailing_whitespace, &fixture).unwrap();

        let other = models.join("other.urdf");
        std::fs::write(
            &other,
            r#"<robot name="b"><mesh filename="/original/base.stl"/></robot>"#,
        )
        .unwrap();
        let error = require_urdf_behind_tool_frame_fixture(&other, &fixture).unwrap_err();
        assert!(error.to_string().contains("fitted to"), "{error}");
    }
}

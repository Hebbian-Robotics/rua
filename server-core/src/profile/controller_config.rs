use std::path::Path;

use anyhow::{Context, ensure};

use super::{Driver, PolicySettings, RobotProfile};
use crate::config::{
    ControllerConfig, ControllerConfigInput, EnableChecks, GripperLimits, GuardConfig,
    MotionLimits, PolicyConfig, SelfCollisionConfig,
};
use crate::so101::calibration::calibrated_joint_limits;
use crate::so101::controller_config::feetech_bus;
use hebbian_contracts::{SimulationModelConfig, SurfaceGuardConfig};

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
                "Profile {} requires an ONNX model for learned control. \
             Provide a compatible model and set [policy] onnx",
                profile.name
            )
        })?;
    Ok(PolicyConfig {
        reach_deadline_seconds: policy.reach_deadline_seconds,
        settle_dwell_seconds: policy.settle_dwell_seconds,
        settled_velocity_radians_per_second: policy.settled_velocity_radians_per_second,
        preview_lag_seconds: policy.preview_lag_seconds,
        onnx: std::fs::canonicalize(onnx)?.display().to_string(),
        max_target_lead_radians: policy.max_target_lead_radians,
        tool_link: profile.arm_tool_frame(arm),
        position_tolerance_meters: policy.position_tolerance_meters,
        orientation_tolerance_radians: policy.orientation_tolerance_radians,
    })
}

fn simulation_model_config(profile: &RobotProfile) -> Option<SimulationModelConfig> {
    matches!(&profile.driver, Driver::Simulator).then(|| SimulationModelConfig {
        effort_unit: profile.effort.unit,
        initial_joint_positions_radians: profile
            .rest_reference
            .as_ref()
            .map(|reference| reference.joint_positions_radians.clone()),
        gripper_force_fraction: profile
            .gripper
            .as_ref()
            .map(|gripper| gripper.force_fraction),
    })
}

pub fn controller_config(
    profile: &RobotProfile,
    arm: &str,
    joint_limits: JointLimitSource,
    calibration_root: &Path,
) -> anyhow::Result<ControllerConfig> {
    ControllerConfig::try_from(controller_config_input(
        profile,
        arm,
        joint_limits,
        calibration_root,
    )?)
    .with_context(|| format!("The config for arm {arm} of {}", profile.name))
}

pub fn controller_config_input(
    profile: &RobotProfile,
    arm: &str,
    joint_limits: JointLimitSource,
    calibration_root: &Path,
) -> anyhow::Result<ControllerConfigInput> {
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
    let config = ControllerConfigInput {
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
        simulation_model: simulation_model_config(profile),
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
        base_pose: profile.base_poses.get(arm).copied(),
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

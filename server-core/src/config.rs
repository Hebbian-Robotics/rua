use std::time::Duration;

use anyhow::{Context, bail};
use hebbian_robot_geometry::joint_limits::JointRecoveryLimits;
use serde::Serialize;

use crate::numeric::positive_clock_duration;

pub use hebbian_contracts::default_trace_directory;
pub use hebbian_contracts::{
    BasePose, EnableChecks, GripperLimits, GuardConfig, MotionLimits, PolicyConfig,
    SelfCollisionConfig, SimPlantConfig, StationDescription,
};
pub use hebbian_so101::config::{
    FeetechConfig, FeetechGripper, FeetechMotor, Gains, MotorCalibration,
};
pub type ControllerConfigInput = hebbian_contracts::ControllerConfigInput<FeetechConfig>;

#[derive(Debug, Clone, Copy)]
pub struct SimTimeouts {
    pub connect: Duration,
    pub reply: Duration,
    pub real_time: bool,
}

impl TryFrom<&SimPlantConfig> for SimTimeouts {
    type Error = anyhow::Error;

    fn try_from(config: &SimPlantConfig) -> anyhow::Result<Self> {
        Ok(Self {
            connect: positive_clock_duration(
                config.connect_timeout_seconds,
                "Simulator connect timeout",
            )?,
            reply: positive_clock_duration(
                config.reply_timeout_seconds,
                "Simulator reply timeout",
            )?,
            real_time: config.real_time,
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ControlTiming {
    pub period: Duration,
    pub maximum_step_seconds: f64,
    pub deadline: Duration,
    pub stream_goal_timeout: Duration,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct GuardTiming {
    pub heartbeat_timeout: Duration,
    pub poll: Duration,
    pub hold_limit: Duration,
}

impl TryFrom<&GuardConfig> for GuardTiming {
    type Error = anyhow::Error;

    fn try_from(config: &GuardConfig) -> anyhow::Result<Self> {
        Ok(Self {
            heartbeat_timeout: positive_clock_duration(
                config.heartbeat_timeout_seconds,
                "Guard heartbeat timeout",
            )?,
            poll: positive_clock_duration(config.poll_seconds, "Guard poll interval")?,
            hold_limit: positive_clock_duration(config.hold_limit_seconds, "Guard hold limit")?,
        })
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(transparent)]
pub struct ControllerConfig {
    input: ControllerConfigInput,
    #[serde(skip)]
    recovery_limits: JointRecoveryLimits,
    #[serde(skip)]
    simulator_timeouts: Option<SimTimeouts>,
    #[serde(skip)]
    pub(crate) control_timing: ControlTiming,
    #[serde(skip)]
    pub(crate) guard_timing: Option<GuardTiming>,
}

impl std::ops::Deref for ControllerConfig {
    type Target = ControllerConfigInput;

    fn deref(&self) -> &Self::Target {
        &self.input
    }
}

impl TryFrom<ControllerConfigInput> for ControllerConfig {
    type Error = anyhow::Error;

    fn try_from(input: ControllerConfigInput) -> anyhow::Result<Self> {
        let recovery_limits = validated_recovery_limits(&input)?;
        let control_timing = ControlTiming {
            period: positive_clock_duration(
                1.0 / input.frequency_hertz,
                "Control period (from frequency_hertz)",
            )?,
            maximum_step_seconds: 2.0 / input.frequency_hertz,
            deadline: positive_clock_duration(
                input.loop_deadline_seconds,
                "Control loop deadline",
            )?,
            stream_goal_timeout: positive_clock_duration(
                input.stream_goal_timeout_seconds,
                "Stream goal timeout",
            )?,
        };
        let guard_timing = input
            .guard
            .as_ref()
            .map(GuardTiming::try_from)
            .transpose()?;
        let simulator_timeouts = input
            .sim_plant
            .as_ref()
            .map(SimTimeouts::try_from)
            .transpose()?;
        Ok(Self {
            input,
            recovery_limits,
            simulator_timeouts,
            control_timing,
            guard_timing,
        })
    }
}

impl ControllerConfig {
    #[must_use]
    pub fn recovery_limits(&self) -> &JointRecoveryLimits {
        &self.recovery_limits
    }

    #[must_use]
    pub fn simulator_timeouts(&self) -> Option<SimTimeouts> {
        self.simulator_timeouts
    }

    #[must_use]
    pub fn into_input(self) -> ControllerConfigInput {
        self.input
    }
    pub fn parse(text: &str) -> anyhow::Result<Self> {
        let config: ControllerConfigInput = serde_json::from_str(text)
            .context("The controller config is not valid JSON for its schema")?;
        Self::try_from(config)
    }
}

fn validate_policy_settings(input: &ControllerConfigInput) -> anyhow::Result<()> {
    if let Some(policy) = &input.policy
        && [
            policy.reach_deadline_seconds,
            policy.settle_dwell_seconds,
            policy.settled_velocity_radians_per_second,
            policy.preview_lag_seconds,
            policy.position_tolerance_meters,
            policy.orientation_tolerance_radians,
        ]
        .iter()
        .any(|value| !value.is_finite() || *value <= 0.0)
    {
        bail!("Policy timings, settling speed and arrival tolerances must be positive and finite");
    }
    Ok(())
}

fn validate_joint_limits(input: &ControllerConfigInput) -> anyhow::Result<JointRecoveryLimits> {
    let n = input.joint_count;
    if n == 0 {
        bail!("An arm needs at least one joint");
    }
    if input.lower_limits.len() != n
        || input.upper_limits.len() != n
        || input.joint_effort_limits.len() != n
    {
        bail!("Per-joint limits must have one value per joint");
    }
    if input
        .joint_effort_limits
        .iter()
        .any(|limit| !limit.is_finite() || *limit <= 0.0)
    {
        bail!("Joint effort limits must be positive and finite");
    }
    JointRecoveryLimits::new(
        &input.lower_limits,
        &input.upper_limits,
        input.joint_limit_recovery_tolerance_radians,
    )
}

fn validated_recovery_limits(input: &ControllerConfigInput) -> anyhow::Result<JointRecoveryLimits> {
    let recovery_limits = validate_joint_limits(input)?;
    let n = input.joint_count;
    let positive = [
        input.tracking_following_lag_seconds,
        input.tracking_static_margin_radians,
        input.max_joint_speed,
        input.default_joint_speed,
        input.motion.acceleration_limit,
        input.motion.jerk_limit,
        input.motion.settled_error_radians,
        input.motion.settle_dwell_seconds,
        input.motion.settle_timeout_seconds,
    ];
    if positive
        .iter()
        .any(|value| !(value.is_finite() && *value > 0.0))
    {
        bail!("Rates, deadlines and caps must be positive and finite");
    }
    if input.default_joint_speed > input.max_joint_speed {
        bail!("The default joint speed exceeds the maximum");
    }
    if let Some(gripper) = &input.gripper {
        if input.driven_joints.len() != n + 1 {
            bail!("A gripper needs one more driven joint than the arm's");
        }
        if !(gripper.closed_radians.is_finite()
            && gripper.open_radians.is_finite()
            && gripper.minimum_radians() < gripper.maximum_radians())
        {
            bail!("The gripper's range is empty or not finite");
        }
    }
    if input.policy.is_some() {
        let expected_joint_count = hebbian_so101::SO101_JOINT_NAMES.len();
        anyhow::ensure!(
            n == expected_joint_count,
            "The policy drives {expected_joint_count} joints"
        );
    }
    validate_policy_settings(input)?;
    if input.sim_plant.is_some() && input.feetech.is_some() {
        bail!("An arm is either a simulator's or a Feetech bus's, not both");
    }
    if let Some(feetech) = &input.feetech {
        if input.guard.is_none() || input.enable_checks.is_none() {
            bail!("A Feetech arm needs the guard and the enable checks configured");
        }
        if feetech.joints.len() != n {
            bail!("The Feetech bus needs one motor per joint");
        }
        feetech.validate()?;
    }
    Ok(recovery_limits)
}

#[cfg(test)]
pub(crate) const TEST_CONFIG: &str = r#"{
  "urdf": "unused.urdf", "driven_joints": ["a", "b"], "surface_guard": null, "self_collision": null, "feetech": null,
  "joint_count": 2, "frequency_hertz": 100.0, "loop_deadline_seconds": 0.1,
  "lower_limits": [-2.0, -2.0], "upper_limits": [2.0, 2.0],
  "joint_effort_limits": [10.0, 10.0], "tracking_following_lag_seconds": 0.04, "tracking_static_margin_radians": 1.5,
  "max_joint_speed": 3.0, "default_joint_speed": 1.5,
  "stream_goal_timeout_seconds": 5.0,
  "joint_limit_recovery_tolerance_radians": 0.0349, "jaw_closing_axis": "x",
  "motion": {"acceleration_limit": 20.0, "jerk_limit": 200.0, "settled_error_radians": 0.02, "settle_dwell_seconds": 0.1, "settle_timeout_seconds": 2.0}
}"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsafe_motion_limits_and_recovery_margins_are_refused() {
        for field in ["acceleration_limit", "jerk_limit", "settle_timeout_seconds"] {
            let mut config = serde_json::from_str::<serde_json::Value>(TEST_CONFIG).unwrap();
            config["motion"][field] = serde_json::json!(0.0);
            assert!(
                ControllerConfig::parse(&config.to_string()).is_err(),
                "{field}"
            );
        }
        let mut config: ControllerConfigInput = serde_json::from_str(TEST_CONFIG).unwrap();
        config.joint_limit_recovery_tolerance_radians = -0.01;
        assert!(ControllerConfig::try_from(config.clone()).is_err());
        config.joint_limit_recovery_tolerance_radians = 0.0;
        assert!(ControllerConfig::try_from(config.clone()).is_ok());
    }

    #[test]
    fn a_consistent_configuration_parses() {
        assert!(ControllerConfig::parse(TEST_CONFIG).is_ok());
    }

    #[test]
    fn simulator_timeouts_must_fit_nonzero_durations() {
        let mut input: ControllerConfigInput = serde_json::from_str(TEST_CONFIG).unwrap();
        input.sim_plant = Some(SimPlantConfig {
            connect_timeout_seconds: 30.0,
            reply_timeout_seconds: 5.0,
            real_time: true,
        });
        assert!(ControllerConfig::try_from(input.clone()).is_ok());

        let mut too_long = input.clone();
        too_long.sim_plant.as_mut().unwrap().connect_timeout_seconds = 1e30;
        let error = ControllerConfig::try_from(too_long).unwrap_err();
        assert!(error.to_string().contains("connect timeout"));

        input.sim_plant.as_mut().unwrap().reply_timeout_seconds = 1e-12;
        let error = ControllerConfig::try_from(input).unwrap_err();
        assert!(error.to_string().contains("reply timeout"));
    }

    #[test]
    fn invalid_control_and_guard_timings_are_rejected_before_running() {
        let input: ControllerConfigInput = serde_json::from_str(TEST_CONFIG).unwrap();
        let mut zero_period = input.clone();
        zero_period.frequency_hertz = 1e30;
        assert!(
            ControllerConfig::try_from(zero_period)
                .unwrap_err()
                .to_string()
                .contains("Control period")
        );

        let mut invalid_guard = input;
        invalid_guard.guard = Some(GuardConfig {
            heartbeat_timeout_seconds: 1e30,
            poll_seconds: 0.05,
            hold_limit_seconds: 30.0,
        });
        assert!(
            ControllerConfig::try_from(invalid_guard)
                .unwrap_err()
                .to_string()
                .contains("Guard heartbeat timeout")
        );
    }

    #[test]
    fn invalid_joint_limits_cannot_become_controller_configuration() {
        let valid: ControllerConfigInput = serde_json::from_str(TEST_CONFIG).unwrap();
        let mut empty = valid.clone();
        empty.joint_count = 0;
        empty.lower_limits.clear();
        empty.upper_limits.clear();
        empty.joint_effort_limits.clear();
        assert!(ControllerConfig::try_from(empty).is_err());

        let mut nonfinite = valid.clone();
        nonfinite.lower_limits[0] = f64::NAN;
        assert!(ControllerConfig::try_from(nonfinite).is_err());

        let mut no_effort = valid;
        no_effort.joint_effort_limits[0] = 0.0;
        assert!(ControllerConfig::try_from(no_effort).is_err());
    }

    #[test]
    fn an_unknown_field_is_refused_so_the_two_sides_cannot_drift() {
        let drifted = TEST_CONFIG.replace("\"joint_count\": 2,", "\"joint_count\": 2, \"new\": 1,");
        assert!(ControllerConfig::parse(&drifted).is_err());
    }

    #[test]
    fn inconsistent_limits_are_refused() {
        let swapped = TEST_CONFIG.replace("[-2.0, -2.0]", "[3.0, 3.0]");
        assert!(ControllerConfig::parse(&swapped).is_err());
        let short = TEST_CONFIG.replace("[10.0, 10.0]", "[10.0]");
        assert!(ControllerConfig::parse(&short).is_err());
    }
    fn feetech_config_json() -> serde_json::Value {
        let motor = |id: u8, sign: i32| {
            serde_json::json!({
                "id": id, "sign": sign, "offset_radians": 0.0,
                "calibration": {"homing_offset": 0, "range_min": 500, "range_max": 3500},
                "expected_gains": {"p": 16, "i": 0, "d": 32}
            })
        };
        let mut config = serde_json::from_str::<serde_json::Value>(TEST_CONFIG).unwrap();
        config["feetech"] = serde_json::json!({
            "port": "unused", "joints": [motor(1, 1), motor(2, -1)], "gripper": null,
            "passive_motor_ids": [], "max_temperature_celsius": 65.0,
            "min_voltage_volts": 10.5, "max_voltage_volts": 13.0
        });
        config["guard"] = serde_json::json!({
            "heartbeat_timeout_seconds": 0.5, "poll_seconds": 0.1, "hold_limit_seconds": 60.0
        });
        config["enable_checks"] = serde_json::json!({
            "preflight_drift_limit_radians": 0.02, "enable_drift_limit_radians": 0.05
        });
        config
    }

    #[test]
    fn a_feetech_configuration_with_its_guard_and_checks_parses() {
        assert!(ControllerConfig::parse(&feetech_config_json().to_string()).is_ok());
    }

    #[test]
    fn a_feetech_arm_without_its_guard_or_enable_checks_is_refused() {
        for missing in ["guard", "enable_checks"] {
            let mut config = feetech_config_json();
            config[missing] = serde_json::Value::Null;
            assert!(
                ControllerConfig::parse(&config.to_string()).is_err(),
                "{missing}"
            );
        }
    }

    #[test]
    fn a_feetech_section_with_a_bad_motor_or_range_is_refused() {
        let mut wrong_count = feetech_config_json();
        wrong_count["feetech"]["joints"]
            .as_array_mut()
            .unwrap()
            .pop();
        let mut bad_sign = feetech_config_json();
        bad_sign["feetech"]["joints"][0]["sign"] = serde_json::json!(2);
        let mut bad_range = feetech_config_json();
        bad_range["feetech"]["joints"][0]["calibration"]["range_min"] = serde_json::json!(4000);
        let mut bad_voltage = feetech_config_json();
        bad_voltage["feetech"]["min_voltage_volts"] = serde_json::json!(14.0);
        for (name, config) in [
            ("motor count", wrong_count),
            ("sign", bad_sign),
            ("range", bad_range),
            ("voltage", bad_voltage),
        ] {
            assert!(
                ControllerConfig::parse(&config.to_string()).is_err(),
                "{name}"
            );
        }
    }
}

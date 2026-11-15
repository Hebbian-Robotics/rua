//! The limits the loop enforces and how the arm is wired, built from the robot's
//! profile by `profile::executor_config` (and, until the station moves to it, by
//! the station). Parsing refuses unknown fields so the sides cannot drift apart
//! silently.

use anyhow::{Context, bail};
use hebbian_contracts::{JawClosingAxis, SurfaceGuardConfig};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SelfCollisionConfig {
    /// Hulls may overlap this deeply before the pair counts as colliding.
    pub penetration_tolerance_meters: f64,
}

/// The jaw angles (the URDF gripper joint's) a gripper goal may ask for.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GripperLimits {
    /// The jaw angle at opening 0.
    pub closed_radians: f64,
    /// The jaw angle at opening 1.
    pub open_radians: f64,
    /// Within this of the target, the jaw has arrived.
    pub position_tolerance_radians: f64,
    /// Short of the target and still for this long, the jaw is holding something.
    pub contact_dwell_seconds: f64,
}

impl GripperLimits {
    #[must_use]
    pub fn minimum_radians(&self) -> f64 {
        self.closed_radians.min(self.open_radians)
    }

    #[must_use]
    pub fn maximum_radians(&self) -> f64 {
        self.closed_radians.max(self.open_radians)
    }

    /// The jaw angle for an opening from 0 (closed) to 1 (open).
    #[must_use]
    pub fn angle(&self, opening: f64) -> f64 {
        self.closed_radians + opening * (self.open_radians - self.closed_radians)
    }

    /// The opening, 0 closed to 1 open, at a jaw angle.
    #[must_use]
    pub fn opening(&self, angle: f64) -> f64 {
        (angle - self.closed_radians) / (self.open_radians - self.closed_radians)
    }
}

/// How planned motions are followed: the trajectory caps between corners, and
/// when a rest point has settled or the settle timeout passes outside tolerance.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MotionLimits {
    pub acceleration_limit: f64,
    pub jerk_limit: f64,
    pub settled_error_radians: f64,
    pub settle_dwell_seconds: f64,
    /// After the reference stops, how long the arm may take to come within
    /// `settled_error_radians` before the move ends as stopped short.
    pub settle_timeout_seconds: f64,
}

/// How the guard process watches the arm if the executor stops: the timings of
/// the profile's `guard` section.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GuardConfig {
    /// Without a heartbeat for this long, the guard takes over.
    pub heartbeat_timeout_seconds: f64,
    /// How often the guard checks a hold it supervises.
    pub poll_seconds: f64,
    /// A supervised hold ends after this long, so an abandoned arm is never
    /// energized forever.
    pub hold_limit_seconds: f64,
}

/// What enabling checks: the arm must not move while it is checked and while torque
/// goes on, so a bumped or backdriven arm is refused rather than held in a new place.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EnableChecks {
    /// The most any joint may move between the preflight and the pose enable holds.
    pub preflight_drift_limit_radians: f64,
    /// The most any joint may move while torque goes on.
    pub enable_drift_limit_radians: f64,
}

/// The learned pose policy, loaded from an ONNX model.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PolicyConfig {
    pub reach_deadline_seconds: f64,
    pub settle_dwell_seconds: f64,
    pub settled_velocity_radians_per_second: f64,
    pub preview_lag_seconds: f64,
    pub onnx: String,
    pub tool_frame_fixture: String,
    /// The checkpoint the model must have been exported from.
    pub checkpoint_sha256: String,
    /// The URDF link whose pose the policy tracks.
    pub tool_link: String,
    /// A reach has arrived when the tool is this close to the goal.
    pub position_tolerance_meters: f64,
    pub orientation_tolerance_radians: f64,
}

// Preserve the station wire format while the SO-101 crate owns its hardware data.
pub use hebbian_so101::config::{
    FeetechConfig, FeetechGripper, FeetechMotor, Gains, MotorCalibration,
};

/// An external simulator stands in for the arm: it connects to the arm's plant
/// socket (`NAME.plant.sock` in the socket directory) and drives time. See
/// `sim_plant.rs`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SimPlantConfig {
    /// How long the executor waits for the simulator to connect at startup.
    pub connect_timeout_seconds: f64,
    /// How long a simulator may take to answer one step before the arm trips.
    pub reply_timeout_seconds: f64,
    /// Whether simulated time follows the wall clock (the default, which the
    /// station's own timeouts assume), or runs as fast as the simulator replies,
    /// for batch runs without clients.
    #[serde(default = "default_real_time")]
    pub real_time: bool,
}

fn default_real_time() -> bool {
    true
}

/// A fixed placement of an arm's base: a position, and a turn about the world's
/// vertical axis.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BasePose {
    pub position_meters: [f64; 3],
    pub yaw_radians: f64,
}

impl BasePose {
    #[must_use]
    pub fn isometry(&self) -> nalgebra::Isometry3<f64> {
        nalgebra::Isometry3::new(
            nalgebra::Vector3::from(self.position_meters),
            nalgebra::Vector3::z() * self.yaw_radians,
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExecutorConfigInput {
    /// The robot description, and the joints the executor drives: the arm's in
    /// order, then the gripper's when it has one.
    pub urdf: String,
    pub driven_joints: Vec<String>,
    /// `None` when the profile turns the table guard off.
    pub surface_guard: Option<SurfaceGuardConfig>,
    /// `None` when the profile turns the self-collision check off.
    pub self_collision: Option<SelfCollisionConfig>,
    /// `None` for a robot without a Feetech bus, such as the simulator.
    pub feetech: Option<FeetechConfig>,
    /// `Some` when an external simulator is the plant, instead of the lag
    /// simulator or the servos.
    #[serde(default)]
    pub sim_plant: Option<SimPlantConfig>,
    /// Required with a Feetech bus; `None` skips the drift checks, as for the simulator.
    #[serde(default)]
    pub enable_checks: Option<EnableChecks>,
    /// Required with a Feetech bus; `None` for a plant with nothing to guard,
    /// such as the simulator.
    #[serde(default)]
    pub guard: Option<GuardConfig>,
    /// The URDF link whose pose a finished command reports as where the arm
    /// ended; without one, records carry joints only.
    #[serde(default)]
    pub tool_link: Option<String>,
    /// Where the arm's URDF root sits in the station's world frame; inverse
    /// kinematics takes and gives tool poses in that frame. `None` when the two
    /// frames are the same.
    #[serde(default)]
    pub base_pose: Option<BasePose>,
    /// `None` when no policy is loaded; pose goals are then refused.
    #[serde(default)]
    pub policy: Option<PolicyConfig>,
    /// `None` when the arm has no gripper; gripper goals are then refused.
    #[serde(default)]
    pub gripper: Option<GripperLimits>,
    /// Where each run writes its cycle trace; always on, this is the only setting.
    #[serde(default = "default_trace_directory")]
    pub trace_directory: String,
    pub motion: MotionLimits,
    pub joint_count: usize,
    pub frequency_hertz: f64,
    pub loop_deadline_seconds: f64,
    pub lower_limits: Vec<f64>,
    pub upper_limits: Vec<f64>,
    pub joint_effort_limits: Vec<f64>,
    /// How long the servos take to follow a moving command: at commanded speed
    /// `v` a joint may trail it by `v * tracking_following_lag_seconds` on top
    /// of the static margin.
    pub tracking_following_lag_seconds: f64,
    /// The tracking error allowed with the command at rest, and the part of the
    /// allowance that does not scale with speed.
    pub tracking_static_margin_radians: f64,
    pub max_joint_speed: f64,
    pub default_joint_speed: f64,
    pub stream_goal_timeout_seconds: f64,
    pub joint_limit_recovery_tolerance_radians: f64,
    /// The tool-frame axis the gripper's jaws close along, which jaw yaw is
    /// measured by.
    pub jaw_closing_axis: JawClosingAxis,
    /// The robot as clients are told about it; none in a bare config.
    #[serde(default)]
    pub station: Option<StationDescription>,
}

/// What clients are told about the robot, the same for every arm.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StationDescription {
    pub robot: String,
    /// Layout, frames and orientation hints for a model.
    pub description: String,
    /// The arm's joints, without any arm prefix, in order.
    pub joint_names: Vec<String>,
}

pub(crate) fn default_trace_directory() -> String {
    "traces/executor".to_owned()
}

/// An executor configuration that has passed its consistency checks.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(transparent)]
pub struct ExecutorConfig(ExecutorConfigInput);

impl std::ops::Deref for ExecutorConfig {
    type Target = ExecutorConfigInput;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl TryFrom<ExecutorConfigInput> for ExecutorConfig {
    type Error = anyhow::Error;

    fn try_from(input: ExecutorConfigInput) -> anyhow::Result<Self> {
        input.validate()?;
        Ok(Self(input))
    }
}

impl ExecutorConfig {
    /// Return to the input form to make changes and validate again.
    #[must_use]
    pub fn into_input(self) -> ExecutorConfigInput {
        self.0
    }
    /// What the arm state machine admits commands by.
    #[must_use]
    pub(crate) fn arm_limits(&self) -> crate::arm::Limits {
        crate::arm::Limits {
            joint_count: self.joint_count,
            lower: self.lower_limits.clone(),
            upper: self.upper_limits.clone(),
            joint_limit_recovery_tolerance_radians: self.joint_limit_recovery_tolerance_radians,
            gripper: self
                .gripper
                .as_ref()
                .map(|limits| (limits.minimum_radians(), limits.maximum_radians())),
            pose_goals: self
                .policy
                .as_ref()
                .map(|policy| crate::arm::PoseGoalSettings {
                    position_tolerance_meters: policy.position_tolerance_meters,
                    orientation_tolerance_radians: policy.orientation_tolerance_radians,
                }),
            jaw_closing_axis: self.jaw_closing_axis,
            default_joint_speed: self.default_joint_speed,
            max_joint_speed: self.max_joint_speed,
        }
    }

    /// # Errors
    /// The text is not a valid, consistent configuration.
    pub fn parse(text: &str) -> anyhow::Result<Self> {
        let config: ExecutorConfigInput = serde_json::from_str(text)
            .context("The executor config is not valid JSON for its schema")?;
        Self::try_from(config)
    }
}

impl ExecutorConfigInput {
    fn validate_policy_settings(&self) -> anyhow::Result<()> {
        if let Some(policy) = &self.policy
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
            bail!(
                "Policy timings, settling speed and arrival tolerances must be positive and finite"
            );
        }
        Ok(())
    }

    fn validate_joint_limits(&self) -> anyhow::Result<()> {
        let n = self.joint_count;
        if n == 0 {
            bail!("An arm needs at least one joint");
        }
        if self.lower_limits.len() != n
            || self.upper_limits.len() != n
            || self.joint_effort_limits.len() != n
        {
            bail!("Per-joint limits must have one value per joint");
        }
        if self
            .lower_limits
            .iter()
            .zip(&self.upper_limits)
            .any(|(lower, upper)| !lower.is_finite() || !upper.is_finite() || lower >= upper)
        {
            bail!("Every joint needs finite limits, with the lower below the upper");
        }
        if self
            .joint_effort_limits
            .iter()
            .any(|limit| !limit.is_finite() || *limit <= 0.0)
        {
            bail!("Joint effort limits must be positive and finite");
        }
        Ok(())
    }

    /// Validate a constructed configuration before starting the control loop.
    ///
    /// # Errors
    /// The configuration is inconsistent or contains invalid values.
    pub fn validate(&self) -> anyhow::Result<()> {
        self.validate_joint_limits()?;
        let n = self.joint_count;
        let positive = [
            self.frequency_hertz,
            self.loop_deadline_seconds,
            self.tracking_following_lag_seconds,
            self.tracking_static_margin_radians,
            self.max_joint_speed,
            self.default_joint_speed,
            self.stream_goal_timeout_seconds,
            self.motion.acceleration_limit,
            self.motion.jerk_limit,
            self.motion.settled_error_radians,
            self.motion.settle_dwell_seconds,
            self.motion.settle_timeout_seconds,
        ];
        if positive
            .iter()
            .any(|value| !(value.is_finite() && *value > 0.0))
        {
            bail!("Rates, deadlines and caps must be positive and finite");
        }
        if !self.joint_limit_recovery_tolerance_radians.is_finite()
            || self.joint_limit_recovery_tolerance_radians < 0.0
        {
            bail!("Joint-limit recovery tolerance must be finite and nonnegative");
        }
        if self.default_joint_speed > self.max_joint_speed {
            bail!("The default joint speed exceeds the maximum");
        }
        if let Some(gripper) = &self.gripper {
            if self.driven_joints.len() != n + 1 {
                bail!("A gripper needs one more driven joint than the arm's");
            }
            if !(gripper.closed_radians.is_finite()
                && gripper.open_radians.is_finite()
                && gripper.minimum_radians() < gripper.maximum_radians())
            {
                bail!("The gripper's range is empty or not finite");
            }
        }
        if self.policy.is_some() {
            crate::so101::policy::validate_joint_count(n)?;
        }
        self.validate_policy_settings()?;
        if let Some(sim) = &self.sim_plant {
            if self.feetech.is_some() {
                bail!("An arm is either a simulator's or a Feetech bus's, not both");
            }
            if !(sim.connect_timeout_seconds.is_finite()
                && sim.connect_timeout_seconds > 0.0
                && sim.reply_timeout_seconds.is_finite()
                && sim.reply_timeout_seconds > 0.0)
            {
                bail!("The simulator timeouts must be positive and finite");
            }
        }
        if let Some(feetech) = &self.feetech {
            if self.guard.is_none() || self.enable_checks.is_none() {
                bail!("A Feetech arm needs the guard and the enable checks configured");
            }
            if feetech.joints.len() != n {
                bail!("The Feetech bus needs one motor per joint");
            }
            feetech.validate()?;
        }
        Ok(())
    }
}

/// A two-joint configuration for tests.
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
                ExecutorConfig::parse(&config.to_string()).is_err(),
                "{field}"
            );
        }
        let mut config: ExecutorConfigInput = serde_json::from_str(TEST_CONFIG).unwrap();
        config.joint_limit_recovery_tolerance_radians = -0.01;
        assert!(config.validate().is_err());
        config.joint_limit_recovery_tolerance_radians = 0.0;
        assert!(config.validate().is_ok());
    }

    #[test]
    fn a_consistent_configuration_parses() {
        assert!(ExecutorConfig::parse(TEST_CONFIG).is_ok());
    }

    #[test]
    fn invalid_joint_limits_cannot_become_executor_configuration() {
        let valid: ExecutorConfigInput = serde_json::from_str(TEST_CONFIG).unwrap();
        let mut empty = valid.clone();
        empty.joint_count = 0;
        empty.lower_limits.clear();
        empty.upper_limits.clear();
        empty.joint_effort_limits.clear();
        assert!(ExecutorConfig::try_from(empty).is_err());

        let mut nonfinite = valid.clone();
        nonfinite.lower_limits[0] = f64::NAN;
        assert!(ExecutorConfig::try_from(nonfinite).is_err());

        let mut no_effort = valid;
        no_effort.joint_effort_limits[0] = 0.0;
        assert!(ExecutorConfig::try_from(no_effort).is_err());
    }

    #[test]
    fn an_unknown_field_is_refused_so_the_two_sides_cannot_drift() {
        let drifted = TEST_CONFIG.replace("\"joint_count\": 2,", "\"joint_count\": 2, \"new\": 1,");
        assert!(ExecutorConfig::parse(&drifted).is_err());
    }

    #[test]
    fn inconsistent_limits_are_refused() {
        let swapped = TEST_CONFIG.replace("[-2.0, -2.0]", "[3.0, 3.0]");
        assert!(ExecutorConfig::parse(&swapped).is_err());
        let short = TEST_CONFIG.replace("[10.0, 10.0]", "[10.0]");
        assert!(ExecutorConfig::parse(&short).is_err());
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
        assert!(ExecutorConfig::parse(&feetech_config_json().to_string()).is_ok());
    }

    #[test]
    fn a_feetech_arm_without_its_guard_or_enable_checks_is_refused() {
        for missing in ["guard", "enable_checks"] {
            let mut config = feetech_config_json();
            config[missing] = serde_json::Value::Null;
            assert!(
                ExecutorConfig::parse(&config.to_string()).is_err(),
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
                ExecutorConfig::parse(&config.to_string()).is_err(),
                "{name}"
            );
        }
    }
}

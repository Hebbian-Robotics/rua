use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{Envelope, Frame, JawClosingAxis, SurfaceGuardConfig};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SelfCollisionConfig {
    /// Hulls may overlap this deeply before the pair counts as colliding.
    pub penetration_tolerance_meters: f64,
    /// Link contacts permitted without a geometry query, in either pair order.
    #[serde(default)]
    pub allowed_collision_pairs: Vec<[String; 2]>,
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
    /// The furthest ahead of a measured joint the model may set its target each cycle.
    pub max_target_lead_radians: f64,
    /// The URDF link whose pose the policy tracks.
    pub tool_link: String,
    /// A reach has arrived when the tool is this close to the goal.
    pub position_tolerance_meters: f64,
    pub orientation_tolerance_radians: f64,
}

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

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "ExecutorConfigInput")]
pub struct ExecutorConfigInput<DriverConfig> {
    /// The robot description, and the joints the executor drives: the arm's in
    /// order, then the gripper's when it has one.
    pub urdf: String,
    pub driven_joints: Vec<String>,
    /// `None` when the profile turns the table guard off.
    pub surface_guard: Option<SurfaceGuardConfig>,
    /// `None` when the profile turns the self-collision check off.
    pub self_collision: Option<SelfCollisionConfig>,
    /// `None` for a robot without a Feetech bus, such as the simulator.
    pub feetech: Option<DriverConfig>,
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

#[must_use]
pub fn default_trace_directory() -> String {
    "traces/executor".to_owned()
}

/// An executor configuration that has passed its consistency checks.
#[derive(JsonSchema)]
#[schemars(rename = "ExecutorConfig")]
#[serde(transparent)]
pub struct ExecutorConfigurationSchema<Config> {
    pub input: Config,
}

/// The root of the generated schema; it exists only to carry the three types.
#[derive(JsonSchema)]
#[schemars(rename = "Contract")]
pub struct ExecutorContract<Config> {
    /// What a client sends.
    pub request: Envelope,
    /// What the executor sends back, one per line.
    pub frame: Frame,
    /// What the executor is started with.
    pub config: ExecutorConfigurationSchema<Config>,
}

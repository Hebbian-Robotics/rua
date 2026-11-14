//! A robot's profile, read from `profiles/<name>.toml`: its joints, limits and
//! wiring. A profile may `extend` others (a name or a list, later ones winning);
//! its tables merge over theirs, so shared limits live in one file.
//!
//! `executor_config` turns a profile and one arm into the loop's
//! `ExecutorConfig`. Relative paths in a profile are relative to the repository
//! root, the directory that holds `profiles/`.

mod executor_config;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail, ensure};
use hebbian_contracts::{ControllerKind, JawClosingAxis};
use serde::Deserialize;

pub use crate::so101::calibration::{
    LerobotMotorCalibration, STS3215_RESOLUTION, calibrated_joint_limits, lerobot_calibration_root,
    load_calibration, so_follower_calibration_path,
};
use crate::so101::policy::CONTROL_FREQUENCY_HERTZ as POLICY_CONTROL_HERTZ;
pub use crate::so101::settings::{FeetechSettings, Sign};

pub use executor_config::{
    JointLimitSource, executor_config, executor_config_input, urdf_joint_limits,
};

/// A descriptive per-joint reference; it never commands motion or proves support.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RestReference {
    pub joint_positions_radians: Vec<f64>,
    pub joint_tolerances_radians: Vec<f64>,
}

impl RestReference {
    /// Compare joint feedback only; this cannot establish physical support.
    #[must_use]
    pub fn matches(&self, positions: &[f64]) -> bool {
        positions.len() == self.joint_positions_radians.len()
            && positions.len() == self.joint_tolerances_radians.len()
            && positions
                .iter()
                .zip(&self.joint_positions_radians)
                .zip(&self.joint_tolerances_radians)
                .all(|((position, expected), tolerance)| {
                    position.is_finite() && (position - expected).abs() <= *tolerance
                })
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControlTiming {
    pub frequency_hertz: f64,
    pub loop_deadline_seconds: f64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrackingLimits {
    /// Allowed tracking error = commanded speed * `following_lag_seconds` +
    /// `static_margin_radians`, per joint.
    pub following_lag_seconds: f64,
    pub static_margin_radians: f64,
    pub settled_error_radians: f64,
    pub settle_dwell_seconds: f64,
    pub settle_timeout_seconds: f64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrajectoryLimits {
    pub acceleration_limit: f64,
    pub jerk_limit: f64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MotionRequestLimits {
    pub default_joint_speed: f64,
    pub max_joint_speed: f64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnableChecks {
    pub preflight_drift_limit_radians: f64,
    pub enable_drift_limit_radians: f64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GuardTiming {
    pub heartbeat_timeout_seconds: f64,
    pub poll_seconds: f64,
    pub hold_limit_seconds: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffortUnit {
    NewtonMetres,
    PercentOfRatedLoad,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffortLimits {
    /// The unit of the effort signal the driver reports and these limits use.
    pub unit: EffortUnit,
    pub joint_effort_limits: Vec<f64>,
}

/// One gripper per arm; openings run from 0 (closed) to 1 (open) between these
/// motor angles.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GripperSettings {
    pub closed_position_radians: f64,
    pub open_position_radians: f64,
    /// Grip force cap as a fraction of the motor's rated torque or current.
    pub force_fraction: f64,
    pub position_tolerance_radians: f64,
    pub contact_dwell_seconds: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MujocoIntegrator {
    Euler,
    Implicitfast,
}

/// A `MuJoCo` scene standing in for the hardware. The executor only checks it;
/// engine-specific adapters read it from the profile itself.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MujocoSettings {
    pub arm_model: PathBuf,
    #[serde(default)]
    pub environment: Option<PathBuf>,
    pub start_pose_radians: Vec<f64>,
    pub target_block_position_meters: [f64; 3],
    pub rated_actuator_force: f64,
    #[serde(default)]
    pub timestep_seconds: Option<f64>,
    #[serde(default)]
    pub integrator: Option<MujocoIntegrator>,
    #[serde(default)]
    pub servo_stiffness_scale: Option<f64>,
    #[serde(default)]
    pub gripper_joint: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UrdfNaming {
    /// URDF joint and frame names carry the arm's name, as `<arm>_<joint>`.
    ArmPrefixed,
    Plain,
}

/// Where an arm's URDF base sits in the station's world frame.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BasePose {
    pub position_meters: [f64; 3],
    #[serde(default)]
    pub yaw_radians: f64,
}

fn default_policy_max_joint_speed() -> f64 {
    2.5
}

fn default_policy_position_tolerance() -> f64 {
    0.02
}

fn default_policy_orientation_tolerance() -> f64 {
    0.1
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicySettings {
    pub reach_deadline_seconds: f64,
    pub settle_dwell_seconds: f64,
    pub settled_velocity_radians_per_second: f64,
    pub preview_lag_seconds: f64,
    pub checkpoint_sha256: String,
    pub tool_frame_fixture: PathBuf,
    /// The checkpoint exported to ONNX, which the policy controller runs.
    #[serde(default)]
    pub onnx: Option<PathBuf>,
    #[serde(default = "default_policy_max_joint_speed")]
    pub max_joint_speed: f64,
    #[serde(default = "default_policy_position_tolerance")]
    pub position_tolerance_meters: f64,
    #[serde(default = "default_policy_orientation_tolerance")]
    pub orientation_tolerance_radians: f64,
}

fn enabled() -> bool {
    true
}

fn default_surface_leeway() -> f64 {
    0.01
}

/// A backup that stops the arm being driven down through its table: a
/// horizontal plane in the arm's base frame. On unless a profile turns it off.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurfaceGuardSettings {
    #[serde(default = "enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub surface_height_meters: f64,
    #[serde(default = "default_surface_leeway")]
    pub leeway_meters: f64,
}

impl Default for SurfaceGuardSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            surface_height_meters: 0.0,
            leeway_meters: default_surface_leeway(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DriverKind {
    Feetech,
    Mujoco,
    Simulator,
}

/// The hardware or simulator behind the arms, with its settings.
#[derive(Debug, Clone)]
pub enum Driver {
    Feetech(FeetechSettings),
    Mujoco(MujocoSettings),
    Simulator,
}

fn default_self_collision_tolerance() -> f64 {
    0.01
}

fn ik_controller() -> ControllerKind {
    ControllerKind::Ik
}

fn arm_prefixed() -> UrdfNaming {
    UrdfNaming::ArmPrefixed
}

/// The profile file as written, after its bases are merged in.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileTables {
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    joint_limit_recovery_tolerance_radians: f64,
    #[serde(default)]
    self_collision_check: bool,
    #[serde(default = "default_self_collision_tolerance")]
    self_collision_penetration_tolerance_meters: f64,
    #[serde(default = "ik_controller")]
    controller: ControllerKind,
    #[serde(default)]
    policy: Option<PolicySettings>,
    driver: DriverKind,
    arms: Vec<String>,
    urdf: PathBuf,
    #[serde(default = "arm_prefixed")]
    urdf_naming: UrdfNaming,
    #[serde(default)]
    cameras: BTreeMap<String, String>,
    #[serde(default)]
    base_poses: BTreeMap<String, BasePose>,
    joint_names: Vec<String>,
    #[serde(default)]
    rest_reference: Option<RestReference>,
    tool_frame: String,
    jaw_closing_axis: JawClosingAxis,
    control: ControlTiming,
    tracking: TrackingLimits,
    trajectory: TrajectoryLimits,
    motion: MotionRequestLimits,
    enable: EnableChecks,
    guard: GuardTiming,
    effort: EffortLimits,
    #[serde(default)]
    surface_guard: SurfaceGuardSettings,
    #[serde(default)]
    gripper: Option<GripperSettings>,
    #[serde(default)]
    feetech: Option<FeetechSettings>,
    #[serde(default)]
    mujoco: Option<MujocoSettings>,
}

/// A validated robot profile. With the policy controller selected, the motion
/// speed and loop rate are already the policy's.
#[derive(Debug, Clone)]
pub struct RobotProfile {
    pub name: String,
    /// What an LLM is told about this robot beyond its state.
    pub description: String,
    /// How far outside a calibrated endpoint a measured joint may rest and still
    /// be commanded back in.
    pub joint_limit_recovery_tolerance_radians: f64,
    /// `Some` when the self-collision check is on.
    pub self_collision_penetration_tolerance_meters: Option<f64>,
    /// `Some` when the profile selects the policy controller.
    pub policy: Option<PolicySettings>,
    pub driver: Driver,
    pub arms: Vec<String>,
    /// Absolute.
    pub urdf: PathBuf,
    pub urdf_naming: UrdfNaming,
    /// USB cameras: camera name to a video device path or an `OpenCV` index.
    pub cameras: BTreeMap<String, String>,
    pub base_poses: BTreeMap<String, BasePose>,
    pub joint_names: Vec<String>,
    pub rest_reference: Option<RestReference>,
    pub tool_frame: String,
    pub jaw_closing_axis: JawClosingAxis,
    pub control: ControlTiming,
    pub tracking: TrackingLimits,
    pub trajectory: TrajectoryLimits,
    pub motion: MotionRequestLimits,
    pub enable: EnableChecks,
    pub guard: GuardTiming,
    pub effort: EffortLimits,
    pub surface_guard: SurfaceGuardSettings,
    pub gripper: Option<GripperSettings>,
}

impl RobotProfile {
    #[must_use]
    pub fn joint_count(&self) -> usize {
        self.joint_names.len()
    }

    #[must_use]
    pub fn urdf_name(&self, arm: &str, name: &str) -> String {
        match self.urdf_naming {
            UrdfNaming::ArmPrefixed => format!("{arm}_{name}"),
            UrdfNaming::Plain => name.to_owned(),
        }
    }

    /// Joint names as they appear in the URDF for one arm.
    #[must_use]
    pub fn arm_joint_names(&self, arm: &str) -> Vec<String> {
        self.joint_names
            .iter()
            .map(|joint| self.urdf_name(arm, joint))
            .collect()
    }

    #[must_use]
    pub fn arm_tool_frame(&self, arm: &str) -> String {
        self.urdf_name(arm, &self.tool_frame)
    }

    #[must_use]
    pub fn feetech(&self) -> Option<&FeetechSettings> {
        match &self.driver {
            Driver::Feetech(settings) => Some(settings),
            Driver::Mujoco(_) | Driver::Simulator => None,
        }
    }
}

/// Where profiles live: `$HEBBIAN_PROFILE_DIRECTORY`, else the nearest
/// installed `share/hebbian-robot-use/profiles` or a checkout's `profiles/`
/// above the running binary, else above the working directory.
///
/// # Errors
/// None of those exists.
pub fn default_profile_directory() -> anyhow::Result<PathBuf> {
    if let Some(directory) =
        std::env::var_os(PROFILE_DIRECTORY_VARIABLE).filter(|value| !value.is_empty())
    {
        return Ok(PathBuf::from(directory));
    }
    // A build in target/<profile>/ and an installed copy beside a checkout both
    // find it above the binary; a run from a checkout finds it above the cwd.
    let mut starts = Vec::new();
    if let Ok(binary) = std::env::current_exe() {
        starts.push(binary);
    }
    if let Ok(working_directory) = std::env::current_dir() {
        starts.push(working_directory);
    }
    starts
        .iter()
        .flat_map(|start| start.ancestors())
        .flat_map(|ancestor| {
            [
                ancestor.join("share/hebbian-robot-use/profiles"),
                ancestor.join("profiles"),
            ]
        })
        .find(|candidate| candidate.is_dir())
        .with_context(|| {
            format!(
                "No profiles/ directory above the executor or the working directory; \
                 set {PROFILE_DIRECTORY_VARIABLE} or pass --profile-directory"
            )
        })
}

/// Overrides where profiles are looked up.
pub const PROFILE_DIRECTORY_VARIABLE: &str = "HEBBIAN_PROFILE_DIRECTORY";

/// Override's values win; a table in both keeps the base's other keys. Only
/// top-level tables merge: a value inside one, such as the per-arm ports, is
/// replaced whole, so an extending profile can drop the base's arms.
fn merge_tables(base: &mut toml::Table, overrides: toml::Table) {
    for (key, value) in overrides {
        match (base.get_mut(&key), value) {
            (Some(toml::Value::Table(existing)), toml::Value::Table(section)) => {
                existing.extend(section);
            }
            (_, value) => {
                base.insert(key, value);
            }
        }
    }
}

fn resolve_local_asset(table: &mut toml::Table, key: &str, directory: &Path) {
    if let Some(toml::Value::String(value)) = table.get_mut(key) {
        let path = Path::new(value);
        if path.is_relative() {
            *value = directory.join(path).to_string_lossy().into_owned();
        }
    }
}

/// A profile's tables, after those of the profiles it extends.
fn profile_tables(directory: &Path, name: &str, depth: usize) -> anyhow::Result<toml::Table> {
    ensure!(depth < 16, "Profile {name} extends itself");
    let requested_path = Path::new(name);
    let path = if requested_path.is_absolute() {
        requested_path.to_owned()
    } else {
        directory.join(format!("{name}.toml"))
    };
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("Cannot read the profile {}", path.display()))?;
    let mut tables: toml::Table =
        toml::from_str(&text).with_context(|| format!("Cannot parse {}", path.display()))?;
    if requested_path.is_absolute() {
        let local_directory = path
            .parent()
            .context("Profile file has no parent directory")?;
        resolve_local_asset(&mut tables, "urdf", local_directory);
        if let Some(toml::Value::Table(mujoco)) = tables.get_mut("mujoco") {
            resolve_local_asset(mujoco, "arm_model", local_directory);
            resolve_local_asset(mujoco, "environment", local_directory);
        }
        if let Some(toml::Value::Table(policy)) = tables.get_mut("policy") {
            resolve_local_asset(policy, "onnx", local_directory);
            resolve_local_asset(policy, "tool_frame_fixture", local_directory);
        }
    }
    let bases = match tables.remove("extends") {
        None => Vec::new(),
        Some(toml::Value::String(base)) => vec![base],
        Some(toml::Value::Array(bases)) => bases
            .into_iter()
            .map(|base| match base {
                toml::Value::String(base) => Ok(base),
                other => bail!("{}: extends names a non-string {other}", path.display()),
            })
            .collect::<anyhow::Result<_>>()?,
        Some(other) => bail!(
            "{}: extends must be a name or a list, not {other}",
            path.display()
        ),
    };
    let mut merged = toml::Table::new();
    for base in bases {
        merge_tables(&mut merged, profile_tables(directory, &base, depth + 1)?);
    }
    merge_tables(&mut merged, tables);
    Ok(merged)
}

/// Load and validate a bundled profile name or an absolute local profile file.
///
/// # Errors
/// The profile or one it extends is missing, malformed or inconsistent.
pub fn load_robot_profile(directory: &Path, name: &str) -> anyhow::Result<RobotProfile> {
    let tables = profile_tables(directory, name, 0)?;
    let parsed: ProfileTables = toml::Value::Table(tables)
        .try_into()
        .with_context(|| format!("Profile {name} does not match the profile schema"))?;
    let repository_root = directory.parent().unwrap_or(Path::new("."));
    let profile = validated(parsed, repository_root).with_context(|| format!("Profile {name}"))?;
    Ok(profile)
}

/// Validate a profile given as JSON with the TOML file's keys and no
/// `extends`, such as the station's parsed copy. Relative paths resolve
/// against `relative_to`.
///
/// # Errors
/// The JSON does not match the profile schema or is inconsistent.
pub fn parse_robot_profile_json(text: &str, relative_to: &Path) -> anyhow::Result<RobotProfile> {
    let parsed: ProfileTables =
        serde_json::from_str(text).context("The profile JSON does not match the profile schema")?;
    let name = parsed.name.clone();
    validated(parsed, relative_to).with_context(|| format!("Profile {name}"))
}

fn positive_and_finite(values: &[f64]) -> bool {
    values.iter().all(|value| value.is_finite() && *value > 0.0)
}

fn check_limits(tables: &ProfileTables) -> anyhow::Result<()> {
    let tracking = &tables.tracking;
    ensure!(
        positive_and_finite(&[
            tables.control.frequency_hertz,
            tables.control.loop_deadline_seconds,
            tracking.following_lag_seconds,
            tracking.static_margin_radians,
            tracking.settled_error_radians,
            tracking.settle_dwell_seconds,
            tracking.settle_timeout_seconds,
            tables.trajectory.acceleration_limit,
            tables.trajectory.jerk_limit,
            tables.motion.default_joint_speed,
            tables.motion.max_joint_speed,
            tables.enable.preflight_drift_limit_radians,
            tables.enable.enable_drift_limit_radians,
            tables.guard.heartbeat_timeout_seconds,
            tables.guard.poll_seconds,
            tables.guard.hold_limit_seconds,
        ]) && positive_and_finite(&tables.effort.joint_effort_limits),
        "Rates, limits and timings must be positive and finite"
    );
    ensure!(
        tables.joint_limit_recovery_tolerance_radians.is_finite()
            && tables.joint_limit_recovery_tolerance_radians >= 0.0
            && tables
                .self_collision_penetration_tolerance_meters
                .is_finite()
            && tables.self_collision_penetration_tolerance_meters >= 0.0
            && tables.surface_guard.leeway_meters.is_finite()
            && tables.surface_guard.leeway_meters >= 0.0
            && tables.surface_guard.surface_height_meters.is_finite(),
        "Tolerances must be finite and nonnegative"
    );
    if let Some(gripper) = &tables.gripper {
        ensure!(
            gripper.closed_position_radians.is_finite()
                && gripper.open_position_radians.is_finite(),
            "Gripper open and closed positions must be finite"
        );
        ensure!(
            positive_and_finite(&[
                gripper.force_fraction,
                gripper.position_tolerance_radians,
                gripper.contact_dwell_seconds,
            ]) && gripper.force_fraction <= 1.0,
            "Gripper tolerances and force fraction must be positive; the force at most 1"
        );
    }
    Ok(())
}

fn check_feetech(tables: &ProfileTables, settings: &FeetechSettings) -> anyhow::Result<()> {
    ensure!(
        tables.effort.unit == EffortUnit::PercentOfRatedLoad,
        "Feetech effort limits must use percent_of_rated_load"
    );
    settings.validate_profile_mapping(
        &tables.arms,
        tables.joint_names.len(),
        tables.gripper.is_some(),
    )
}

fn resolve(repository_root: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_owned()
    } else {
        repository_root.join(path)
    }
}

fn check_rest_reference(tables: &ProfileTables) -> anyhow::Result<()> {
    let joint_count = tables.joint_names.len();
    if let Some(reference) = &tables.rest_reference {
        ensure!(
            reference.joint_positions_radians.len() == joint_count
                && reference.joint_tolerances_radians.len() == joint_count
                && reference
                    .joint_positions_radians
                    .iter()
                    .all(|value| value.is_finite())
                && reference
                    .joint_tolerances_radians
                    .iter()
                    .all(|value| value.is_finite() && *value > 0.0),
            "Rest reference needs one finite position and positive tolerance per joint"
        );
    }
    Ok(())
}

fn validated(tables: ProfileTables, repository_root: &Path) -> anyhow::Result<RobotProfile> {
    let joint_count = tables.joint_names.len();
    ensure!(
        !tables.arms.is_empty() && joint_count > 0,
        "A profile needs arms and joints"
    );
    check_rest_reference(&tables)?;
    let mut unique_joints = tables.joint_names.clone();
    unique_joints.sort();
    unique_joints.dedup();
    ensure!(
        unique_joints.len() == joint_count,
        "Joint names must be unique"
    );
    if let Some(arm) = tables
        .base_poses
        .keys()
        .find(|arm| !tables.arms.contains(arm))
    {
        bail!("Base pose for unknown arm {arm}");
    }
    check_limits(&tables)?;
    let driver = match (tables.driver, &tables.feetech, &tables.mujoco) {
        (DriverKind::Simulator, None, None) => Driver::Simulator,
        (DriverKind::Feetech, Some(settings), None) => {
            check_feetech(&tables, settings)?;
            Driver::Feetech(settings.clone())
        }
        (DriverKind::Mujoco, None, Some(settings)) => {
            ensure!(
                settings.start_pose_radians.len() == joint_count,
                "The MuJoCo start pose needs one angle per joint"
            );
            Driver::Mujoco(settings.clone())
        }
        (DriverKind::Feetech, ..) => {
            bail!("A Feetech profile needs a [feetech] section and no [mujoco] one")
        }
        (DriverKind::Simulator, ..) => {
            bail!("A simulator profile must not contain [feetech] or [mujoco] settings")
        }
        (DriverKind::Mujoco, ..) => {
            bail!("A MuJoCo profile needs a [mujoco] section and no [feetech] one")
        }
    };
    let mut motion = tables.motion;
    let mut control = tables.control;
    let policy = match (tables.controller, tables.policy) {
        (ControllerKind::Ik, _) => None,
        (ControllerKind::Policy, Some(policy)) => {
            crate::so101::policy::validate_profile(
                tables.arms.len(),
                &tables.joint_names,
                !tables.base_poses.is_empty(),
            )?;
            // The policy runs the SO-101's five joints at its own rate and speed.
            motion.max_joint_speed = policy.max_joint_speed;
            control.frequency_hertz = POLICY_CONTROL_HERTZ;
            Some(PolicySettings {
                tool_frame_fixture: resolve(repository_root, &policy.tool_frame_fixture),
                onnx: policy.onnx.map(|onnx| resolve(repository_root, &onnx)),
                ..policy
            })
        }
        (ControllerKind::Policy, None) => bail!("The policy controller needs a [policy] section"),
    };
    Ok(RobotProfile {
        name: tables.name,
        description: tables.description,
        joint_limit_recovery_tolerance_radians: tables.joint_limit_recovery_tolerance_radians,
        self_collision_penetration_tolerance_meters: tables
            .self_collision_check
            .then_some(tables.self_collision_penetration_tolerance_meters),
        policy,
        driver,
        arms: tables.arms,
        urdf: resolve(repository_root, &tables.urdf),
        urdf_naming: tables.urdf_naming,
        cameras: tables.cameras,
        base_poses: tables.base_poses,
        joint_names: tables.joint_names,
        rest_reference: tables.rest_reference,
        tool_frame: tables.tool_frame,
        jaw_closing_axis: tables.jaw_closing_axis,
        control,
        tracking: tables.tracking,
        trajectory: tables.trajectory,
        motion,
        enable: tables.enable,
        guard: tables.guard,
        effort: tables.effort,
        surface_guard: tables.surface_guard,
        gripper: tables.gripper,
    })
}

#[cfg(test)]
mod tests;

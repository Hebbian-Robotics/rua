mod executor_config;
mod robot_assembly;

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, bail, ensure};
use hebbian_contracts::{BasePose, ControllerKind, EffortUnit, JawClosingAxis};
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

// SAFETY: A descriptive per-joint reference; it never commands motion or proves support.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RestReference {
    pub joint_positions_radians: Vec<f64>,
    pub joint_tolerances_radians: Vec<f64>,
}

impl RestReference {
    // SAFETY: Compare joint feedback only; this cannot establish physical support.
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

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControlTiming {
    pub frequency_hertz: f64,
    pub loop_deadline_seconds: f64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrackingLimits {
    pub following_lag_seconds: f64,
    pub static_margin_radians: f64,
    pub settled_error_radians: f64,
    pub settle_dwell_seconds: f64,
    pub settle_timeout_seconds: f64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrajectoryLimits {
    pub acceleration_limit: f64,
    pub jerk_limit: f64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffortLimits {
    pub unit: EffortUnit,
    pub joint_effort_limits: Vec<f64>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GripperSettings {
    pub closed_position_radians: f64,
    pub open_position_radians: f64,
    pub force_fraction: f64,
    pub position_tolerance_radians: f64,
    pub contact_dwell_seconds: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UrdfNaming {
    ArmPrefixed,
    Plain,
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
    // CONSTRAINT: The furthest ahead of a measured joint the model may set its target each cycle
    // (the model's action step must not exceed it). A lead the servo chases, not a speed.
    pub max_target_lead_radians: f64,
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

#[derive(Debug, Clone, PartialEq, Deserialize)]
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
    Simulator,
}

#[derive(Debug, Clone)]
pub enum Driver {
    Feetech(Box<FeetechSettings>),
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
    #[serde(default)]
    allowed_collision_pairs: Vec<[String; 2]>,
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
}

#[derive(Debug, Clone)]
pub struct RobotProfile {
    pub name: String,
    pub description: String,
    pub joint_limit_recovery_tolerance_radians: f64,
    pub self_collision_penetration_tolerance_meters: Option<f64>,
    pub allowed_collision_pairs: Vec<[String; 2]>,
    pub policy: Option<PolicySettings>,
    pub driver: Driver,
    pub arms: Vec<String>,
    pub urdf: PathBuf,
    pub urdf_naming: UrdfNaming,
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
            Driver::Simulator => None,
        }
    }
}

pub fn default_profile_directory() -> anyhow::Result<PathBuf> {
    if let Some(directory) =
        std::env::var_os(PROFILE_DIRECTORY_VARIABLE).filter(|value| !value.is_empty())
    {
        return Ok(PathBuf::from(directory));
    }
    // WHY: A build in target/<profile>/ and an installed copy beside a checkout both
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
                "No profiles/ directory above the server binary or the working directory; \
                 set {PROFILE_DIRECTORY_VARIABLE} or pass --profile-directory"
            )
        })
}

pub const PROFILE_DIRECTORY_VARIABLE: &str = "HEBBIAN_PROFILE_DIRECTORY";

// CONSTRAINT: Override's values win; a table in both keeps the base's other keys. Only
// top-level tables merge: a value inside one, such as the per-arm ports, is
// replaced whole, so an extending profile can drop the base's arms.
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

fn profile_tables(directory: &Path, name: &str, depth: usize) -> anyhow::Result<toml::Table> {
    ensure!(
        depth < 16,
        "Profile {name} exceeds the inheritance limit of 16 profiles"
    );
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
        if let Some(toml::Value::Table(policy)) = tables.get_mut("policy") {
            resolve_local_asset(policy, "onnx", local_directory);
        }
        if let Some(toml::Value::Table(model)) = tables.get_mut("robot_assembly") {
            resolve_local_asset(model, "manifest", local_directory);
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

pub fn load_robot_profile(directory: &Path, name: &str) -> anyhow::Result<RobotProfile> {
    load_robot_profile_with_fragments(directory, name, &[])
}

pub fn load_robot_profile_with_fragments(
    directory: &Path,
    name: &str,
    fragments: &[&str],
) -> anyhow::Result<RobotProfile> {
    let mut tables = profile_tables(directory, name, 0)?;
    for fragment in fragments {
        merge_tables(&mut tables, profile_tables(directory, fragment, 0)?);
    }
    let repository_root = directory.parent().unwrap_or(Path::new("."));
    robot_assembly::apply_robot_assembly(&mut tables, repository_root)
        .with_context(|| format!("Profile {name}"))?;
    let parsed: ProfileTables = toml::Value::Table(tables)
        .try_into()
        .with_context(|| format!("Profile {name} does not match the profile schema"))?;
    let profile = validated(parsed, repository_root).with_context(|| format!("Profile {name}"))?;
    Ok(profile)
}

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

pub fn validate_arm_names<'a>(names: impl IntoIterator<Item = &'a str>) -> anyhow::Result<()> {
    let mut unique_names = HashSet::new();
    for name in names {
        ensure!(
            !name.is_empty()
                && name.bytes().all(|character| {
                    character.is_ascii_alphanumeric() || matches!(character, b'_' | b'-')
                }),
            "Invalid arm name {name:?}: use letters, digits, underscores or hyphens"
        );
        ensure!(unique_names.insert(name), "Duplicate arm name {name:?}");
    }
    Ok(())
}

fn validated(tables: ProfileTables, repository_root: &Path) -> anyhow::Result<RobotProfile> {
    let joint_count = tables.joint_names.len();
    ensure!(
        !tables.arms.is_empty() && joint_count > 0,
        "A profile needs arms and joints"
    );
    validate_arm_names(tables.arms.iter().map(String::as_str))?;
    check_rest_reference(&tables)?;
    let mut unique_joints = tables.joint_names.clone();
    unique_joints.sort();
    unique_joints.dedup();
    ensure!(
        unique_joints.len() == joint_count,
        "Joint names must be unique"
    );
    for (arm, base) in &tables.base_poses {
        ensure!(tables.arms.contains(arm), "Base pose for unknown arm {arm}");
        ensure!(
            base.position_meters
                .iter()
                .all(|coordinate| coordinate.is_finite())
                && base.yaw_radians.is_finite(),
            "Base pose for {arm} must contain finite coordinates and yaw"
        );
    }
    check_limits(&tables)?;
    let driver = match (tables.driver, &tables.feetech) {
        (DriverKind::Simulator, None) => Driver::Simulator,
        (DriverKind::Feetech, Some(settings)) => {
            check_feetech(&tables, settings)?;
            Driver::Feetech(Box::new(settings.clone()))
        }
        (DriverKind::Feetech, None) => bail!("A Feetech profile needs a [feetech] section"),
        (DriverKind::Simulator, Some(_)) => {
            bail!("A simulator profile must not contain [feetech] settings")
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
            // CONSTRAINT: The policy runs the SO-101's five joints at its own rate and speed.
            motion.max_joint_speed = policy.max_joint_speed;
            control.frequency_hertz = POLICY_CONTROL_HERTZ;
            Some(PolicySettings {
                onnx: policy.onnx.map(|onnx| resolve(repository_root, &onnx)),
                ..policy
            })
        }
        (ControllerKind::Policy, None) => bail!("Learned control requires a [policy] section"),
    };
    Ok(RobotProfile {
        name: tables.name,
        description: tables.description,
        joint_limit_recovery_tolerance_radians: tables.joint_limit_recovery_tolerance_radians,
        self_collision_penetration_tolerance_meters: tables
            .self_collision_check
            .then_some(tables.self_collision_penetration_tolerance_meters),
        allowed_collision_pairs: tables.allowed_collision_pairs,
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

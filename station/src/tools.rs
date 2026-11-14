//! The commands besides `run`: inspecting and releasing servos, comparing the
//! kinematics and the policy against their references, the guard process, and
//! the schemas.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::Context;
use hebbian_robot_use_server::config::ExecutorConfig;
use hebbian_robot_use_server::kinematics::pose_from_xyzw;
use hebbian_robot_use_server::plant::Plant;
use hebbian_robot_use_server::profile;
use hebbian_robot_use_server::robot::Robot;
use hebbian_robot_use_server::so101::driver::{SerialBus, Servos};
use hebbian_robot_use_server::so101::policy::{self, Policy};
use serde::Deserialize;

use crate::run::bus_timing_json;

pub(crate) fn load_config(path: &Path) -> anyhow::Result<ExecutorConfig> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("Cannot read the config {}", path.display()))?;
    ExecutorConfig::parse(&text).with_context(|| format!("Invalid config {}", path.display()))
}

pub(crate) fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> anyhow::Result<T> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("Cannot read {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("Cannot parse {}", path.display()))
}

pub(crate) fn feetech_servos(config: &ExecutorConfig) -> anyhow::Result<Servos<SerialBus>> {
    let bus = config
        .feetech
        .clone()
        .context("This command needs a config with a feetech section")?;
    Servos::open(bus)
}

/// The servo registers `check-arm` reads and shows, never writes: the gains the
/// profile pins, and the startup, dead-zone, torque and speed settings that shape
/// how a servo starts to move. All are in the STS3215 table.
const SERVO_TUNING_REGISTERS: &[&str] = &[
    "p_coefficient",
    "i_coefficient",
    "d_coefficient",
    "acceleration",
    "maximum_acceleration",
    "acceleration_multiplier",
    "minimum_startup_force",
    "cw_dead_zone",
    "ccw_dead_zone",
    "return_delay_time",
    "torque_limit",
    "max_torque_limit",
    "protective_torque",
    "overload_torque",
    "protection_current",
    "protection_time",
    "goal_speed",
    "maximum_velocity_limit",
    "operating_mode",
    "unloading_condition",
];

fn print_register_table(ids: &[u8], registers: &[(String, Result<Vec<i64>, String>)]) {
    eprintln!(
        "{:<26}{}",
        "register",
        ids.iter()
            .map(|id| format!("{:>8}", format!("id {id}")))
            .collect::<Vec<String>>()
            .concat()
    );
    for (name, values) in registers {
        match values {
            Ok(values) => eprintln!(
                "{name:<26}{}",
                values
                    .iter()
                    .map(|value| format!("{value:>8}"))
                    .collect::<Vec<String>>()
                    .concat()
            ),
            Err(error) => eprintln!("{name:<26}unreadable: {error}"),
        }
    }
}

pub(crate) fn check_arm(config_path: &Path) -> anyhow::Result<()> {
    let config = load_config(config_path)?;
    let mut servos = feetech_servos(&config)?;
    servos.preflight()?;
    let feedback = servos.read_feedback()?;
    let registers = servos.dump_registers(SERVO_TUNING_REGISTERS);
    print_register_table(servos.driven_ids(), &registers);
    let register_values: serde_json::Map<String, serde_json::Value> = registers
        .into_iter()
        .map(|(name, values)| {
            let value = match values {
                Ok(values) => serde_json::json!(values),
                Err(error) => serde_json::json!({ "error": error }),
            };
            (name, value)
        })
        .collect();
    println!(
        "{}",
        serde_json::json!({
            "motor_ids": servos.driven_ids(),
            "registers": register_values,
            "positions": feedback.positions,
            "efforts": feedback.efforts,
            "temperatures_celsius": feedback.temperatures_celsius,
            "gripper_angle": feedback.gripper_angle,
            "bus": bus_timing_json(servos.bus().timing()),
        })
    );
    Ok(())
}

#[derive(Deserialize)]
struct RobotProbe {
    urdf: PathBuf,
    driven_joints: Vec<String>,
    samples: Vec<Vec<f64>>,
}

/// Print the lowest point of the moving geometry for each sample, for comparing
/// the table guard's model of the arm with another one.
pub(crate) fn probe_robot(trace_path: &Path) -> anyhow::Result<()> {
    let probe: RobotProbe = read_json(trace_path)?;
    let robot = Robot::load(&probe.urdf, &probe.driven_joints)?;
    let lowest = probe
        .samples
        .iter()
        .map(|joints| robot.lowest_point(joints))
        .collect::<anyhow::Result<Vec<f64>>>()?;
    println!("{}", serde_json::json!(lowest));
    Ok(())
}

#[derive(Deserialize)]
struct PolicyReplayStep {
    reset: bool,
    positions: [f64; policy::JOINT_COUNT],
    velocities: [f64; policy::JOINT_COUNT],
    target_position: [f64; 3],
    target_quaternion: [f64; 4],
}

#[derive(Deserialize)]
struct PolicyReplay {
    urdf: PathBuf,
    arm_joints: Vec<String>,
    tool_link: String,
    onnx: PathBuf,
    tool_frame_fixture: PathBuf,
    checkpoint_sha256: String,
    steps: Vec<PolicyReplayStep>,
}

/// Run recorded observations through the policy and print each step's action and
/// joint target, for comparing against a reference implementation.
pub(crate) fn replay_policy(trace_path: &Path) -> anyhow::Result<()> {
    let replay: PolicyReplay = read_json(trace_path)?;
    let robot = Robot::load(&replay.urdf, &replay.arm_joints)?;
    let mut network = Policy::load(
        &replay.onnx,
        &replay.tool_frame_fixture,
        &replay.checkpoint_sha256,
    )?;
    let results = replay
        .steps
        .iter()
        .map(|step| {
            if step.reset {
                network.reset();
            }
            let tool = robot.link_pose(&replay.tool_link, &step.positions)?;
            let target = pose_from_xyzw(step.target_position, step.target_quaternion)?;
            let output = network.step(&step.positions, &step.velocities, &tool, &target)?;
            Ok(serde_json::json!({
                "action": output.action,
                "joint_target": output.joint_target,
            }))
        })
        .collect::<anyhow::Result<Vec<serde_json::Value>>>()?;
    println!("{}", serde_json::Value::Array(results));
    Ok(())
}

/// Turn torque off on the arm in this config, which drops it.
pub(crate) fn disable_arm(config_path: &Path) -> anyhow::Result<()> {
    feetech_servos(&load_config(config_path)?)?.release()
}

/// Check that a config file parses and is consistent.
pub(crate) fn check_config(config_path: &Path) -> anyhow::Result<()> {
    load_config(config_path).map(drop)
}

pub(crate) fn print_profile_config(
    profile_name: Option<&str>,
    selection: &crate::so101::SelectionArguments,
    arm: &str,
    profile_directory: Option<&Path>,
    urdf_limits: bool,
) -> anyhow::Result<()> {
    let robot_profile = if let Some(profile_name) = profile_name {
        let directory = match profile_directory {
            Some(directory) => directory.to_owned(),
            None => profile::default_profile_directory()?,
        };
        selection.load_profile(profile_name, &directory)?
    } else {
        selection.require_explicit_profile()?;
        let text = std::io::read_to_string(std::io::stdin()).context("Cannot read stdin")?;
        profile::parse_robot_profile_json(&text, &std::env::current_dir()?)?
    };
    let joint_limits = if urdf_limits {
        profile::JointLimitSource::Urdf
    } else {
        profile::JointLimitSource::Calibration
    };
    let config = profile::executor_config(
        &robot_profile,
        arm,
        joint_limits,
        &profile::lerobot_calibration_root(),
    )?;
    println!("{}", serde_json::to_string_pretty(&config)?);
    Ok(())
}

/// Watch an enabled arm for the executor that started this process.
pub(crate) fn run_guard(config_path: &Path, executor_pid: u32) -> anyhow::Result<()> {
    let config = load_config(config_path)?;
    let feetech = config
        .feetech
        .clone()
        .context("The guard needs a config with a feetech section")?;
    let outcome =
        hebbian_robot_use_server::guard::guard(&config, executor_pid, std::io::stdin(), || {
            Servos::open(feetech.clone())
        })?;
    // stderr reaches the operator's terminal, like the guard's other reports.
    let _ = writeln!(
        std::io::stderr().lock(),
        "{}",
        serde_json::json!({ "guard": outcome.to_string() })
    );
    Ok(())
}

/// Which document `schema` writes.
pub(crate) enum SchemaKind {
    Contract,
    SimPlant,
    OpenApi,
}

/// Write a JSON Schema or the `OpenAPI` document to stdout.
pub(crate) fn print_schema(kind: SchemaKind) {
    let schema = match kind {
        SchemaKind::SimPlant => hebbian_robot_use_server::sim_plant::schema_json(),
        SchemaKind::OpenApi => hebbian_station::openapi::openapi_json(),
        SchemaKind::Contract => hebbian_robot_use_server::contract::schema_json(),
    };
    print!("{schema}");
}

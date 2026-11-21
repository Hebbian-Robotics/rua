#![allow(
    clippy::disallowed_types,
    clippy::disallowed_methods,
    reason = "Explicit maintenance commands inspect registers and release hardware independently of station motion."
)]

use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::Context;
use hebbian_robot_use_core::config::ControllerConfig;
use hebbian_robot_use_core::kinematics::pose_from_xyzw;
use hebbian_robot_use_core::profile;
use hebbian_robot_use_core::robot::Robot;
use hebbian_robot_use_core::so101::driver::{SerialBus, Servos};
use hebbian_robot_use_core::so101::policy::{self, Policy};
use serde::Deserialize;

use crate::run::bus_timing_json;

pub(crate) fn load_config(path: &Path) -> anyhow::Result<ControllerConfig> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("Cannot read the config {}", path.display()))?;
    ControllerConfig::parse(&text).with_context(|| format!("Invalid config {}", path.display()))
}

pub(crate) fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> anyhow::Result<T> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("Cannot read {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("Cannot parse {}", path.display()))
}

pub(crate) fn feetech_servos(config: &ControllerConfig) -> anyhow::Result<Servos<SerialBus>> {
    let bus = config
        .feetech
        .clone()
        .context("This command needs a config with a feetech section")?;
    Servos::open(bus)
}

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
    steps: Vec<PolicyReplayStep>,
}

pub(crate) fn replay_policy(trace_path: &Path) -> anyhow::Result<()> {
    let replay: PolicyReplay = read_json(trace_path)?;
    let robot = Robot::load(&replay.urdf, &replay.arm_joints)?;
    let mut network = Policy::load(&replay.onnx, &replay.urdf)?;
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

pub(crate) fn disable_arm(config_path: &Path) -> anyhow::Result<()> {
    feetech_servos(&load_config(config_path)?)?.release()
}

pub(crate) fn check_config(config_path: &Path) -> anyhow::Result<()> {
    load_config(config_path).map(drop)
}

pub(crate) fn print_profile_config(
    profile_name: &str,
    selection: &crate::commands::SelectionArguments,
    arm: &str,
    profile_directory: Option<&Path>,
    urdf_limits: bool,
) -> anyhow::Result<()> {
    let directory = match profile_directory {
        Some(directory) => directory.to_owned(),
        None => profile::default_profile_directory()?,
    };
    let robot_profile = selection.load_profile(profile_name, &directory, false)?;
    let joint_limits = if urdf_limits {
        profile::JointLimitSource::Urdf
    } else {
        profile::JointLimitSource::Calibration
    };
    let config = profile::controller_config(
        &robot_profile,
        arm,
        joint_limits,
        &profile::lerobot_calibration_root(),
    )?;
    println!("{}", serde_json::to_string_pretty(&config)?);
    Ok(())
}

pub(crate) fn run_guard(config_path: &Path, controller_pid: u32) -> anyhow::Result<()> {
    let config = load_config(config_path)?;
    let feetech = config
        .feetech
        .clone()
        .context("The guard needs a config with a feetech section")?;
    let outcome =
        hebbian_robot_use_core::guard::guard(&config, controller_pid, std::io::stdin(), || {
            Servos::open(feetech.clone())
        })?;
    let _ = writeln!(
        std::io::stderr().lock(),
        "{}",
        serde_json::json!({ "guard": outcome.to_string() })
    );
    Ok(())
}

pub(crate) enum SchemaKind {
    SimPlant,
    OpenApi,
}

pub(crate) fn print_schema(kind: SchemaKind) {
    let schema = match kind {
        SchemaKind::SimPlant => hebbian_robot_use_core::sim_plant::schema_json(),
        SchemaKind::OpenApi => hebbian_robot_use_server::openapi::openapi_json(),
    };
    print!("{schema}");
}

//! The learned SO-101 pose policy, run from its ONNX export.
//!
//! The ONNX graph takes the raw
//! goal (21) and raw history (150) and returns the unclamped action (5); this
//! module builds those inputs, keeps the history, and applies the clamp and the
//! 0.04 rad step. Model assets are supplied separately from the controller.

use std::path::Path;
use std::sync::Mutex;

use anyhow::{Context, ensure};
use nalgebra::{Isometry3, Matrix3, Translation3, UnitQuaternion};
use ort::session::Session;
use ort::value::Tensor;
use serde::Deserialize;

pub const JOINT_COUNT: usize = super::SO101_JOINT_NAMES.len();
const FRAME_SIZE: usize = 15;
const HISTORY_FRAMES: usize = 10;
const HISTORY_SIZE: usize = FRAME_SIZE * HISTORY_FRAMES;
const GOAL_SIZE: usize = 21;
/// The network's contract: measured joints plus 0.04 rad times the clamped action.
pub(crate) const ACTION_STEP_RADIANS: f64 = 0.04;
/// Timing is part of this trained controller's action and history contract.
pub(crate) const CONTROL_FREQUENCY_HERTZ: f64 = 50.0;
const METHOD: &str = "supermimic_so101_reference_pose_v2";

/// The rigid transform from the training model's tool site to the station's tool
/// frame, as written in the checkpoint's tool-frame fixture.
#[derive(Deserialize)]
struct ToolFrameFixture {
    transform_station_from_simulation_tool: [[f64; 4]; 4],
}

/// The station tool pose and the target, mapped into the training model's frame.
fn simulation_pose(
    station_from_simulation: &Isometry3<f64>,
    station: &Isometry3<f64>,
) -> Isometry3<f64> {
    station * station_from_simulation.inverse()
}

fn rigid_transform(matrix: &[[f64; 4]; 4]) -> anyhow::Result<Isometry3<f64>> {
    let rotation = Matrix3::from_fn(|row, column| matrix[row][column]);
    let tolerance = 1e-10;
    ensure!(
        matrix.iter().flatten().all(|value| value.is_finite())
            && matrix[3]
                .iter()
                .zip([0.0, 0.0, 0.0, 1.0])
                .all(|(value, wanted)| (value - wanted).abs() < tolerance)
            && (rotation.transpose() * rotation - Matrix3::identity())
                .abs()
                .max()
                < tolerance
            && (rotation.determinant() - 1.0).abs() < tolerance,
        "The policy tool-frame fixture is not a finite rigid transform"
    );
    Ok(Isometry3::from_parts(
        Translation3::new(matrix[0][3], matrix[1][3], matrix[2][3]),
        UnitQuaternion::from_matrix(&rotation),
    ))
}

/// ONNX Runtime allows one environment per process, and a process holds more
/// than one policy (each arm's control loop, and its ghost's rehearsal).
fn runtime_environment() -> anyhow::Result<ort::environment::Environment> {
    static ENVIRONMENT: Mutex<Option<ort::environment::Environment>> = Mutex::new(None);
    let mut shared = ENVIRONMENT
        .lock()
        .expect("the environment's lock is not poisoned");
    if let Some(environment) = shared.as_ref() {
        return Ok(environment.clone());
    }
    let library_path = std::env::var_os("ORT_DYLIB_PATH")
        .filter(|value| !value.is_empty())
        .map_or_else(
            || {
                format!(
                    "{}onnxruntime{}",
                    std::env::consts::DLL_PREFIX,
                    std::env::consts::DLL_SUFFIX
                )
                .into()
            },
            std::path::PathBuf::from,
        );
    let environment = ort::init_from(&library_path)
        .with_context(|| {
            format!(
                "Cannot load ONNX Runtime from {}; set ORT_DYLIB_PATH to a compatible library",
                library_path.display()
            )
        })?
        .build()
        .context("Cannot start ONNX Runtime")?;
    *shared = Some(environment.clone());
    Ok(environment)
}

/// What the policy produced for one cycle.
pub struct PolicyStep {
    /// The network's raw output, before the clamp.
    pub action: [f64; JOINT_COUNT],
    /// Measured joints + 0.04 * clamp(action). The loop's guards and joint
    /// limits still apply to it.
    pub joint_target: [f64; JOINT_COUNT],
}

pub struct Policy {
    session: Session,
    station_from_simulation_tool: Isometry3<f64>,
    observation_joint_offset: [f64; JOINT_COUNT],
    previous_action: [f64; JOINT_COUNT],
    history: Option<Vec<f64>>,
}

impl Policy {
    /// Interpret live feedback using this model's five-joint observation layout.
    pub(crate) fn joint_goal(
        &mut self,
        feedback: &crate::plant::Feedback,
        robot: &crate::robot::Robot,
        tool_link: &str,
        target: &Isometry3<f64>,
    ) -> anyhow::Result<[f64; JOINT_COUNT]> {
        let positions: [f64; JOINT_COUNT] = feedback
            .positions
            .as_slice()
            .try_into()
            .context("The policy needs five measured joints")?;
        let velocities: [f64; JOINT_COUNT] = feedback
            .velocities
            .as_slice()
            .try_into()
            .context("The policy needs five measured velocities")?;
        let tool = robot.pose_at(tool_link, &feedback.positions, feedback.gripper_angle)?;
        Ok(self
            .step(&positions, &velocities, &tool, target)?
            .joint_target)
    }

    /// Load the exported network and refuse a model whose contract differs.
    /// `expected_checkpoint_sha256` pins the checkpoint the model came from.
    ///
    /// # Errors
    /// The model cannot load, its metadata names another contract or checkpoint,
    /// or the fixture is not a rigid transform.
    pub fn load(
        onnx_path: &Path,
        tool_frame_fixture_path: &Path,
        expected_checkpoint_sha256: &str,
    ) -> anyhow::Result<Self> {
        let environment = runtime_environment()?;
        let session = Session::builder(&environment)
            .and_then(|mut builder| builder.commit_from_file(onnx_path))
            .with_context(|| format!("Cannot load the policy model {}", onnx_path.display()))?;
        let observation_joint_offset = Self::check_metadata(&session, expected_checkpoint_sha256)?;
        let fixture: ToolFrameFixture = serde_json::from_str(
            &std::fs::read_to_string(tool_frame_fixture_path)
                .with_context(|| format!("Cannot read {}", tool_frame_fixture_path.display()))?,
        )
        .context("Cannot parse the tool-frame fixture")?;
        Ok(Self {
            session,
            station_from_simulation_tool: rigid_transform(
                &fixture.transform_station_from_simulation_tool,
            )?,
            observation_joint_offset,
            previous_action: [0.0; JOINT_COUNT],
            history: None,
        })
    }

    fn check_metadata(
        session: &Session,
        expected_checkpoint_sha256: &str,
    ) -> anyhow::Result<[f64; JOINT_COUNT]> {
        let metadata = session
            .metadata()
            .context("The policy model has no metadata")?;
        let custom = |key: &str| -> anyhow::Result<String> {
            metadata
                .custom(key)
                .with_context(|| format!("The policy model has no `{key}` metadata"))
        };
        ensure!(
            custom("method")? == METHOD,
            "Unsupported SO-101 policy observation contract"
        );
        ensure!(
            custom("action_mode")? == "measured_increment"
                && custom("action_step")?
                    .parse::<f64>()
                    .is_ok_and(|step| (step - ACTION_STEP_RADIANS).abs() < 1e-12),
            "The SO-101 policy must use 0.04 rad measured-increment actions"
        );
        ensure!(
            custom("checkpoint_sha256")? == expected_checkpoint_sha256,
            "The policy model comes from a different checkpoint than the config pins"
        );
        let offset: [f64; JOINT_COUNT] = serde_json::from_str(&custom("observation_joint_offset")?)
            .context("Invalid `observation_joint_offset` metadata")?;
        ensure!(
            offset.iter().all(|value| value.is_finite()),
            "The policy's joint offset must be finite"
        );
        Ok(offset)
    }

    /// Forget the history and the previous action, as at the start of a rollout.
    pub fn reset(&mut self) {
        self.previous_action = [0.0; JOINT_COUNT];
        self.history = None;
    }

    /// One policy cycle. `tool` is the station's tool pose for `positions` (the
    /// executor's forward kinematics) and `target` the station-frame goal.
    ///
    /// # Errors
    /// An input is not finite, or the network fails or returns a non-finite action.
    pub fn step(
        &mut self,
        positions: &[f64; JOINT_COUNT],
        velocities: &[f64; JOINT_COUNT],
        tool: &Isometry3<f64>,
        target: &Isometry3<f64>,
    ) -> anyhow::Result<PolicyStep> {
        ensure!(
            positions
                .iter()
                .chain(velocities)
                .all(|value| value.is_finite())
                && tool
                    .translation
                    .vector
                    .iter()
                    .all(|value| value.is_finite())
                && target
                    .translation
                    .vector
                    .iter()
                    .all(|value| value.is_finite()),
            "Policy inputs must be finite"
        );
        let current = simulation_pose(&self.station_from_simulation_tool, tool);
        let wanted = simulation_pose(&self.station_from_simulation_tool, target);

        let frame: Vec<f64> = positions
            .iter()
            .zip(&self.observation_joint_offset)
            .map(|(position, offset)| position - offset)
            .chain(velocities.iter().copied())
            .chain(self.previous_action)
            .collect();
        let history = match self.history.take() {
            None => frame.repeat(HISTORY_FRAMES),
            Some(mut history) => {
                history.drain(..FRAME_SIZE);
                history.extend(&frame);
                history
            }
        };
        let mut goal = Vec::with_capacity(GOAL_SIZE);
        goal.extend(current.translation.vector.iter());
        goal.extend(wanted.translation.vector.iter());
        goal.extend([0.0; 3]);
        // The first two columns of each rotation matrix, row by row.
        for rotation in [current.rotation, wanted.rotation] {
            let matrix = rotation.to_rotation_matrix();
            for row in 0..3 {
                goal.extend([matrix[(row, 0)], matrix[(row, 1)]]);
            }
        }
        debug_assert_eq!(goal.len(), GOAL_SIZE);
        debug_assert_eq!(history.len(), HISTORY_SIZE);

        let action = self.run_network(&goal, &history)?;
        self.history = Some(history);
        self.previous_action = action;
        let mut joint_target = [0.0; JOINT_COUNT];
        for ((target, position), raw) in joint_target.iter_mut().zip(positions).zip(&action) {
            *target = position + ACTION_STEP_RADIANS * raw.clamp(-1.0, 1.0);
        }
        Ok(PolicyStep {
            action,
            joint_target,
        })
    }

    fn run_network(&mut self, goal: &[f64], history: &[f64]) -> anyhow::Result<[f64; JOINT_COUNT]> {
        let to_tensor = |values: &[f64]| {
            Tensor::from_array((
                [1_usize, values.len()],
                values
                    .iter()
                    .map(|value| {
                        #[allow(clippy::cast_possible_truncation)]
                        let narrowed = *value as f32;
                        narrowed
                    })
                    .collect::<Vec<f32>>(),
            ))
        };
        let outputs = self.session.run(ort::inputs![
            "goal" => to_tensor(goal)?,
            "history" => to_tensor(history)?,
        ])?;
        let (_, values) = outputs["action"].try_extract_tensor::<f32>()?;
        ensure!(
            values.len() == JOINT_COUNT,
            "The policy returned {} values",
            values.len()
        );
        let mut action = [0.0; JOINT_COUNT];
        for (slot, value) in action.iter_mut().zip(values) {
            ensure!(
                value.is_finite(),
                "The SO-101 policy produced a non-finite action"
            );
            *slot = f64::from(*value);
        }
        Ok(action)
    }
}

/// Reject an embodiment layout this trained model cannot observe or command.
pub(crate) fn validate_joint_count(joint_count: usize) -> anyhow::Result<()> {
    ensure!(
        joint_count == JOINT_COUNT,
        "The policy drives {JOINT_COUNT} joints"
    );
    Ok(())
}

/// Rate and action scale are part of the trained model, not shared loop policy.
pub(crate) fn validate_execution(config: &crate::config::ExecutorConfig) -> anyhow::Result<()> {
    validate_joint_count(config.joint_count)?;
    ensure!(
        (config.frequency_hertz - CONTROL_FREQUENCY_HERTZ).abs() < 1e-9,
        "The selected SO-101 policy requires a {CONTROL_FREQUENCY_HERTZ} Hz control rate"
    );
    ensure!(
        config.max_joint_speed >= ACTION_STEP_RADIANS * config.frequency_hertz,
        "The selected policy's speed ceiling cannot accommodate its trained action step"
    );
    Ok(())
}

/// The current policy was trained for one SO-101 in its original base frame.
pub(crate) fn validate_profile(
    arm_count: usize,
    joint_names: &[String],
    has_base_poses: bool,
) -> anyhow::Result<()> {
    ensure!(
        arm_count == 1 && joint_names == super::SO101_JOINT_NAMES && !has_base_poses,
        "Policy selection requires SO-101 policy settings and an untransformed base"
    );
    Ok(())
}

use std::path::Path;
use std::sync::Mutex;
use std::time::Instant;

use anyhow::{Context, ensure};
use nalgebra::Isometry3;
use ort::session::Session;
use ort::value::Tensor;

pub const JOINT_COUNT: usize = super::SO101_JOINT_NAMES.len();
const FRAME_SIZE: usize = 3 * JOINT_COUNT;
const HISTORY_FRAMES: usize = 10;
const HISTORY_SIZE: usize = FRAME_SIZE * HISTORY_FRAMES;
const GOAL_SIZE: usize = 21;
struct ActionStep(f64);

impl ActionStep {
    fn parse(metadata: &str) -> anyhow::Result<Self> {
        let radians: f64 = metadata.parse().context("Invalid `action_step` metadata")?;
        ensure!(
            radians.is_finite() && radians > 0.0,
            "The policy's action step must be finite and positive"
        );
        Ok(Self(radians))
    }

    fn joint_target(&self, measured_position: f64, action: f64) -> f64 {
        measured_position + self.0 * action.clamp(-1.0, 1.0)
    }

    // CONSTRAINT: The step is how far ahead of the measured joint the policy may set its target. It is
    // a lead the servo chases, not a joint speed, so the profile accepts it as a distance.
    fn validate_target_lead(&self, max_target_lead_radians: f64) -> anyhow::Result<()> {
        ensure!(
            self.0 <= max_target_lead_radians + 1e-12,
            "The policy model sets targets up to {} rad ahead of the measured joints, but the profile accepts at most {max_target_lead_radians} rad ([policy] max_target_lead_radians)",
            self.0
        );
        Ok(())
    }
}
// CONSTRAINT: Timing is part of this trained controller's action and history contract.
pub(crate) const CONTROL_FREQUENCY_HERTZ: f64 = 50.0;
const METHOD: &str = "supermimic_so101_reference_pose_v2";

struct HardwareBinding {
    hardware: String,
    station_urdf_sha256: String,
}

impl HardwareBinding {
    // SAFETY: The policy's goals are poses of the station tool frame of the URDF it was trained on;
    // refuse an arm configured with another URDF.
    fn require_urdf(&self, arm_urdf: &Path, arm_urdf_sha256: &str) -> anyhow::Result<()> {
        ensure!(
            self.station_urdf_sha256 == arm_urdf_sha256,
            "The policy model is released for {} (URDF sha256 {}), but the arm is configured with {} (sha256 {arm_urdf_sha256})",
            self.hardware,
            self.station_urdf_sha256,
            arm_urdf.display()
        );
        Ok(())
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    use std::fmt::Write;
    Sha256::digest(bytes)
        .iter()
        .fold(String::with_capacity(64), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        })
}

// CONSTRAINT: ONNX Runtime allows one environment per process, and a process holds more
// than one policy (each arm's control loop, and its ghost's rehearsal).
fn runtime_environment() -> anyhow::Result<ort::environment::Environment> {
    static ENVIRONMENT: Mutex<Option<ort::environment::Environment>> = Mutex::new(None);
    let mut shared = ENVIRONMENT
        .lock()
        .map_err(|_| anyhow::anyhow!("The policy runtime environment lock is poisoned"))?;
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

pub struct PolicyStep {
    pub action: [f64; JOINT_COUNT],
    // SAFETY: Measured joints + the model's action step * clamp(action). The loop's guards and joint
    // limits still apply to it.
    pub joint_target: [f64; JOINT_COUNT],
}

pub struct Policy {
    session: Session,
    action_step: ActionStep,
    observation_joint_offset: [f64; JOINT_COUNT],
    previous_action: [f64; JOINT_COUNT],
    history: Option<Vec<f64>>,
    pub(crate) inference_seconds: f64,
}

impl Policy {
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

    pub fn load(
        onnx_path: &Path,
        arm_urdf: &Path,
        expected_checkpoint_sha256: &str,
    ) -> anyhow::Result<Self> {
        let environment = runtime_environment()?;
        let session = Session::builder(&environment)
            .and_then(|mut builder| builder.commit_from_file(onnx_path))
            .with_context(|| format!("Cannot load the policy model {}", onnx_path.display()))?;
        let (action_step, observation_joint_offset) =
            Self::check_metadata(&session, expected_checkpoint_sha256)?;
        if let Some(binding) = Self::hardware_binding(&session)? {
            let arm_urdf_bytes = std::fs::read(arm_urdf)
                .with_context(|| format!("Cannot read the URDF {}", arm_urdf.display()))?;
            binding.require_urdf(arm_urdf, &sha256_hex(&arm_urdf_bytes))?;
        }
        Ok(Self {
            session,
            action_step,
            observation_joint_offset,
            previous_action: [0.0; JOINT_COUNT],
            history: None,
            inference_seconds: 0.0,
        })
    }

    fn check_metadata(
        session: &Session,
        expected_checkpoint_sha256: &str,
    ) -> anyhow::Result<(ActionStep, [f64; JOINT_COUNT])> {
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
            custom("action_mode")? == "measured_increment",
            "The SO-101 policy must use measured-increment actions"
        );
        let action_step = ActionStep::parse(&custom("action_step")?)?;
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
        Ok((action_step, offset))
    }

    fn hardware_binding(session: &Session) -> anyhow::Result<Option<HardwareBinding>> {
        let metadata = session
            .metadata()
            .context("The policy model has no metadata")?;
        let Some(hardware) = metadata.custom("hardware") else {
            return Ok(None);
        };
        let custom = |key: &str| -> anyhow::Result<String> {
            metadata.custom(key).with_context(|| {
                format!("The policy model names hardware {hardware} but has no `{key}` metadata")
            })
        };
        Ok(Some(HardwareBinding {
            station_urdf_sha256: custom("station_urdf_sha256")?,
            hardware,
        }))
    }

    pub(crate) fn validate_target_lead(&self, max_target_lead_radians: f64) -> anyhow::Result<()> {
        self.action_step
            .validate_target_lead(max_target_lead_radians)
    }

    pub fn reset(&mut self) {
        self.previous_action = [0.0; JOINT_COUNT];
        self.history = None;
    }

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

        let relative_positions =
            std::array::from_fn(|joint| positions[joint] - self.observation_joint_offset[joint]);
        let frame = [relative_positions, *velocities, self.previous_action];
        let frame = frame.as_flattened();
        let history = match self.history.take() {
            None => frame.repeat(HISTORY_FRAMES),
            Some(mut history) => {
                history.drain(..FRAME_SIZE);
                history.extend_from_slice(frame);
                history
            }
        };
        let mut goal = [0.0; GOAL_SIZE];
        goal[..3].copy_from_slice(tool.translation.vector.as_slice());
        goal[3..6].copy_from_slice(target.translation.vector.as_slice());
        // CONSTRAINT: The first two columns of each rotation matrix, row by row.
        for (rotation_features, rotation) in goal[9..]
            .as_chunks_mut::<6>()
            .0
            .iter_mut()
            .zip([tool.rotation, target.rotation])
        {
            let matrix = rotation.to_rotation_matrix();
            for (row, columns) in rotation_features
                .as_chunks_mut::<2>()
                .0
                .iter_mut()
                .enumerate()
            {
                *columns = [matrix[(row, 0)], matrix[(row, 1)]];
            }
        }
        debug_assert_eq!(history.len(), HISTORY_SIZE);

        let action = self.run_network(&goal, &history)?;
        self.history = Some(history);
        self.previous_action = action;
        let mut joint_target = [0.0; JOINT_COUNT];
        for ((target, position), raw) in joint_target.iter_mut().zip(positions).zip(&action) {
            *target = self.action_step.joint_target(*position, *raw);
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
        let began = Instant::now();
        let inference = self.session.run(ort::inputs![
            "goal" => to_tensor(goal)?,
            "history" => to_tensor(history)?,
        ]);
        self.inference_seconds = began.elapsed().as_secs_f64();
        let outputs = inference?;
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

pub(crate) fn validate_joint_count(joint_count: usize) -> anyhow::Result<()> {
    ensure!(
        joint_count == JOINT_COUNT,
        "The policy drives {JOINT_COUNT} joints"
    );
    Ok(())
}

// CONSTRAINT: Rate and action scale are part of the trained model, not shared loop policy.
pub(crate) fn validate_execution(config: &crate::config::ExecutorConfig) -> anyhow::Result<()> {
    validate_joint_count(config.joint_count)?;
    ensure!(
        (config.frequency_hertz - CONTROL_FREQUENCY_HERTZ).abs() < 1e-9,
        "The selected SO-101 policy requires a {CONTROL_FREQUENCY_HERTZ} Hz control rate"
    );
    Ok(())
}

// CONSTRAINT: The current policy was trained for one SO-101 in its original base frame.
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

#[cfg(test)]
mod tests {
    use super::{ActionStep, HardwareBinding, sha256_hex};
    use std::path::Path;

    #[test]
    fn checkpoint_step_scales_targets_and_must_fit_the_accepted_target_lead() {
        let original_step = ActionStep::parse("0.04").unwrap();
        let retrained_step = ActionStep::parse("0.12").unwrap();
        original_step.validate_target_lead(0.04).unwrap();
        assert!(retrained_step.validate_target_lead(0.04).is_err());
        retrained_step.validate_target_lead(0.12).unwrap();
        for (action, original_target, retrained_target) in [
            (-2.0, 0.96, 0.88),
            (0.0, 1.0, 1.0),
            (0.5, 1.02, 1.06),
            (2.0, 1.04, 1.12),
        ] {
            assert!((original_step.joint_target(1.0, action) - original_target).abs() < 1e-12);
            assert!((retrained_step.joint_target(1.0, action) - retrained_target).abs() < 1e-12);
        }
        for invalid in ["", "not-a-number", "NaN", "inf", "-inf", "0", "-0.12"] {
            assert!(ActionStep::parse(invalid).is_err(), "accepted {invalid}");
        }
    }

    #[test]
    fn a_hardware_bound_model_refuses_an_arm_configured_with_another_urdf() {
        let binding = HardwareBinding {
            hardware: "so101-soft-gripper-camera".into(),
            station_urdf_sha256: sha256_hex(b"<robot name=\"soft\"/>"),
        };
        let arm_urdf = Path::new("robot.urdf");
        binding
            .require_urdf(arm_urdf, &sha256_hex(b"<robot name=\"soft\"/>"))
            .unwrap();
        assert!(
            binding
                .require_urdf(arm_urdf, &sha256_hex(b"<robot name=\"stock\"/>"))
                .is_err()
        );
    }
}

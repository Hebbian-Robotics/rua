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
}

// CONSTRAINT: Per-joint vectors in the model's metadata, as JSON arrays of five numbers.
fn parse_joint_vector(name: &str, metadata: &str) -> anyhow::Result<[f64; JOINT_COUNT]> {
    let values: [f64; JOINT_COUNT] =
        serde_json::from_str(metadata).with_context(|| format!("Invalid `{name}` metadata"))?;
    ensure!(
        values.iter().all(|value| value.is_finite()),
        "The policy's `{name}` must be finite"
    );
    Ok(values)
}

// CONSTRAINT: The trained model's action metadata defines the target mapping: the default pose
// plus a per-joint scale times the action, no further than the step from the measured joint.
// WHY: An absolute target lets a constant action hold the arm still as it settles.
struct ActionMapping {
    step: ActionStep,
    offset: [f64; JOINT_COUNT],
    scale: [f64; JOINT_COUNT],
}

impl ActionMapping {
    fn parse(step: &str, offset: &str, scale: &str) -> anyhow::Result<Self> {
        let scale = parse_joint_vector("action_scale_radians", scale)?;
        ensure!(
            scale.iter().all(|value| *value > 0.0),
            "The policy's action scale must be positive"
        );
        Ok(Self {
            step: ActionStep::parse(step)?,
            offset: parse_joint_vector("action_offset_radians", offset)?,
            scale,
        })
    }

    fn joint_target(&self, joint: usize, measured_position: f64, action: f64) -> f64 {
        (self.offset[joint] + self.scale[joint] * action).clamp(
            measured_position - self.step.0,
            measured_position + self.step.0,
        )
    }

    // CONSTRAINT: The step is how far ahead of the measured joint the policy may set its target. It is
    // a lead the servo chases, not a joint speed, so the profile accepts it as a distance.
    fn validate_target_lead(&self, max_target_lead_radians: f64) -> anyhow::Result<()> {
        let step = self.step.0;
        ensure!(
            step <= max_target_lead_radians + 1e-12,
            "The policy model sets targets up to {step} rad ahead of the measured joints, but the profile accepts at most {max_target_lead_radians} rad ([policy] max_target_lead_radians)"
        );
        Ok(())
    }
}
// CONSTRAINT: Timing is part of this trained controller's action and history contract.
pub(crate) const CONTROL_FREQUENCY_HERTZ: f64 = 50.0;
const METHOD: &str = "supermimic_official_so101_pose_v1";
const ACTION_MODE: &str = "default_pose_offset";

struct HardwareBinding {
    hardware: String,
    station_urdf_sha256: String,
}

impl HardwareBinding {
    // CONSTRAINT: The policy's goals are poses of the station tool frame of the URDF it was trained on;
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
    // ROBOT-SAFETY: At most the model's action step from the measured joints (ActionMapping). The loop's
    // guards and joint limits still apply to it.
    pub joint_target: [f64; JOINT_COUNT],
}

pub struct Policy {
    session: Session,
    action_mapping: ActionMapping,
    observation_joint_offset: [f64; JOINT_COUNT],
    previous_action: [f64; JOINT_COUNT],
    history: Option<Vec<f64>>,
    inference_seconds: f64,
}

impl Policy {
    pub fn load(onnx_path: &Path, arm_urdf: &Path) -> anyhow::Result<Self> {
        let environment = runtime_environment()?;
        let session = Session::builder(&environment)
            .and_then(|mut builder| builder.commit_from_file(onnx_path))
            .with_context(|| format!("Cannot load the policy model {}", onnx_path.display()))?;
        let (action_mapping, observation_joint_offset) = Self::check_metadata(&session)?;
        if let Some(binding) = Self::hardware_binding(&session)? {
            let arm_urdf_bytes = std::fs::read(arm_urdf)
                .with_context(|| format!("Cannot read the URDF {}", arm_urdf.display()))?;
            binding.require_urdf(arm_urdf, &sha256_hex(&arm_urdf_bytes))?;
        }
        Ok(Self {
            session,
            action_mapping,
            observation_joint_offset,
            previous_action: [0.0; JOINT_COUNT],
            history: None,
            inference_seconds: 0.0,
        })
    }

    fn check_metadata(session: &Session) -> anyhow::Result<(ActionMapping, [f64; JOINT_COUNT])> {
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
        let action_mode = custom("action_mode")?;
        ensure!(
            action_mode == ACTION_MODE,
            "Unsupported SO-101 policy action mode `{action_mode}`"
        );
        let action_mapping = ActionMapping::parse(
            &custom("action_step")?,
            &custom("action_offset_radians")?,
            &custom("action_scale_radians")?,
        )?;
        let offset = parse_joint_vector(
            "observation_joint_offset",
            &custom("observation_joint_offset")?,
        )?;
        Ok((action_mapping, offset))
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
        self.action_mapping
            .validate_target_lead(max_target_lead_radians)
    }

    pub fn reset(&mut self) {
        self.previous_action = [0.0; JOINT_COUNT];
        self.history = None;
    }

    pub(crate) fn take_inference_seconds(&mut self) -> f64 {
        std::mem::take(&mut self.inference_seconds)
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
        for (joint, ((target, position), raw)) in joint_target
            .iter_mut()
            .zip(positions)
            .zip(&action)
            .enumerate()
        {
            *target = self.action_mapping.joint_target(joint, *position, *raw);
        }
        Ok(PolicyStep {
            action,
            joint_target,
        })
    }

    fn run_network(&mut self, goal: &[f64], history: &[f64]) -> anyhow::Result<[f64; JOINT_COUNT]> {
        let to_tensor = |name: &str, features: &[f64]| -> anyhow::Result<_> {
            let features = policy_float32_features(name, features)?;
            Ok(Tensor::from_array(([1, features.len()], features))?)
        };
        let began = Instant::now();
        let goal = to_tensor("goal", goal)?;
        let history = to_tensor("history", history)?;
        let inference = self
            .session
            .run(ort::inputs!["goal" => goal, "history" => history]);
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

fn policy_float32_features(name: &str, features: &[f64]) -> anyhow::Result<Vec<f32>> {
    let mut input = Vec::with_capacity(features.len());
    for (index, feature) in features.iter().enumerate() {
        #[allow(
            clippy::cast_possible_truncation,
            reason = "The policy model expects float32 tensors; rounding is expected."
        )]
        let narrowed = *feature as f32;
        ensure!(
            narrowed.is_finite(),
            "Policy input {name}[{index}] is not finite in float32"
        );
        input.push(narrowed);
    }
    Ok(input)
}

// CONSTRAINT: Rate and action scale are part of the trained model, not shared loop policy.
pub(crate) fn validate_control_rate(frequency_hertz: f64) -> anyhow::Result<()> {
    ensure!(
        (frequency_hertz - CONTROL_FREQUENCY_HERTZ).abs() < 1e-9,
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
    use super::{ActionMapping, ActionStep, HardwareBinding, policy_float32_features, sha256_hex};
    use std::path::Path;

    fn mapping(step: &str) -> ActionMapping {
        ActionMapping::parse(step, "[0.1, 0, 0, 0, 0]", "[0.4, 0.4, 0.4, 0.4, 0.4]").unwrap()
    }

    #[test]
    fn constant_action_holds_the_target_within_the_configured_joint_lead() {
        let mapping = mapping("0.12");
        for measured in [0.45, 0.5, 0.55] {
            assert!((mapping.joint_target(0, measured, 1.0) - 0.5).abs() < 1e-12);
        }
        assert!((mapping.joint_target(1, 0.0, 5.0) - 0.12).abs() < 1e-12);
        assert!((mapping.joint_target(1, 0.0, -5.0) + 0.12).abs() < 1e-12);
        mapping.validate_target_lead(0.12).unwrap();
        assert!(mapping.validate_target_lead(0.04).is_err());
        for invalid in ["", "not-a-number", "NaN", "inf", "-inf", "0", "-0.12"] {
            assert!(ActionStep::parse(invalid).is_err(), "accepted {invalid}");
        }
        for (offset, scale) in [
            ("[0, 0, 0, 0]", "[1, 1, 1, 1, 1]"),
            ("[0, 0, 0, 0, 0]", "[1, 1, 1, 1, 0]"),
            ("[0, 0, 0, 0, NaN]", "[1, 1, 1, 1, 1]"),
        ] {
            assert!(
                ActionMapping::parse("0.12", offset, scale).is_err(),
                "accepted {offset} {scale}"
            );
        }
    }

    #[test]
    fn policy_refuses_finite_features_that_overflow_float32() {
        for (name, feature) in [("goal", f64::MAX), ("history", -f64::MAX)] {
            let error = policy_float32_features(name, &[0.25, feature]).unwrap_err();
            assert_eq!(
                error.to_string(),
                format!("Policy input {name}[1] is not finite in float32")
            );
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

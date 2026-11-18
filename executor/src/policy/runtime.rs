use std::path::Path;
use std::sync::Arc;

use anyhow::Context;

use nalgebra::Isometry3;

use crate::config::ExecutorConfig;
use crate::plant::Feedback;
use crate::robot::Robot;
use crate::so101::policy::Policy;

pub(crate) struct PolicyRuntime {
    policy: Policy,
    robot: Arc<Robot>,
    tool_link: String,
}

impl PolicyRuntime {
    pub(crate) fn new(
        config: &ExecutorConfig,
        robot: Option<&Arc<Robot>>,
    ) -> anyhow::Result<Option<Self>> {
        let Some(settings) = &config.policy else {
            return Ok(None);
        };
        crate::so101::policy::validate_execution(config)?;
        let policy = Policy::load(
            Path::new(&settings.onnx),
            Path::new(&settings.tool_frame_fixture),
            &settings.checkpoint_sha256,
        )?;
        policy.validate_target_lead(settings.max_target_lead_radians)?;
        Ok(Some(Self {
            policy,
            robot: Arc::clone(robot.context("A policy needs the robot model")?),
            tool_link: settings.tool_link.clone(),
        }))
    }

    pub(crate) fn begin_cycle(&mut self) {
        self.policy.inference_seconds = 0.0;
    }

    pub(crate) fn inference_seconds(&self) -> f64 {
        self.policy.inference_seconds
    }

    pub(crate) fn reset(&mut self) {
        self.policy.reset();
    }

    pub(crate) fn tool_pose(&self, feedback: &Feedback) -> anyhow::Result<Isometry3<f64>> {
        self.robot
            .pose_at(&self.tool_link, &feedback.positions, feedback.gripper_angle)
    }

    pub(crate) fn joint_goal(
        &mut self,
        config: &ExecutorConfig,
        feedback: &Feedback,
        target: &Isometry3<f64>,
    ) -> anyhow::Result<Vec<f64>> {
        let joint_target =
            self.policy
                .joint_goal(feedback, &self.robot, &self.tool_link, target)?;
        Ok(joint_target
            .iter()
            .zip(config.lower_limits.iter().zip(&config.upper_limits))
            .map(|(value, (lower, upper))| value.clamp(*lower, *upper))
            .collect())
    }
}

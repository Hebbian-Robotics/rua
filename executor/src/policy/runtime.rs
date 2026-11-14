//! Learned-policy execution shared by the live loop and offline preview.

use std::path::Path;
use std::sync::Arc;

use anyhow::Context;

use nalgebra::Isometry3;

use crate::config::ExecutorConfig;
use crate::plant::Feedback;
use crate::robot::Robot;
use crate::so101::policy::Policy;

/// The policy and what it needs to see the arm: its forward kinematics.
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
        Ok(Some(Self {
            policy: Policy::load(
                Path::new(&settings.onnx),
                Path::new(&settings.tool_frame_fixture),
                &settings.checkpoint_sha256,
            )?,
            robot: Arc::clone(robot.context("A policy needs the robot model")?),
            tool_link: settings.tool_link.clone(),
        }))
    }

    /// Begin an independent reach or pose stream without carrying old history.
    pub(crate) fn reset(&mut self) {
        self.policy.reset();
    }

    /// The tool's pose at the measured joints.
    pub(crate) fn tool_pose(&self, feedback: &Feedback) -> anyhow::Result<Isometry3<f64>> {
        self.robot
            .pose_at(&self.tool_link, &feedback.positions, feedback.gripper_angle)
    }

    /// The joint goal one policy step takes from the measured state toward the
    /// pose, held inside the joint limits (the common guards still follow).
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

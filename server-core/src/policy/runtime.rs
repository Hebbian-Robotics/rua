use std::path::Path;
use std::sync::Arc;

use anyhow::Context;

use nalgebra::Isometry3;

use crate::config::ControllerConfig;
use crate::plant::Feedback;
use crate::robot::Robot;
use crate::so101::policy::{JOINT_COUNT, Policy};

pub(crate) struct PolicyRuntime {
    policy: Policy,
    robot: Arc<Robot>,
    tool_link: String,
}

impl PolicyRuntime {
    pub(crate) fn new(
        config: &ControllerConfig,
        robot: Option<&Arc<Robot>>,
    ) -> anyhow::Result<Option<Self>> {
        let Some(settings) = &config.policy else {
            return Ok(None);
        };
        crate::so101::policy::validate_control_rate(config.frequency_hertz)?;
        let policy = Policy::load(Path::new(&settings.onnx), Path::new(&config.urdf))?;
        policy.validate_target_lead(settings.max_target_lead_radians)?;
        Ok(Some(Self {
            policy,
            robot: Arc::clone(robot.context("A policy needs the robot model")?),
            tool_link: settings.tool_link.clone(),
        }))
    }

    pub(crate) fn take_inference_seconds(&mut self) -> f64 {
        self.policy.take_inference_seconds()
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
        config: &ControllerConfig,
        feedback: &Feedback,
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
        let tool_pose = self.tool_pose(feedback)?;
        let mut joint_target = self
            .policy
            .step(&positions, &velocities, &tool_pose, target)?
            .joint_target;
        for ((target, lower), upper) in joint_target
            .iter_mut()
            .zip(&config.lower_limits)
            .zip(&config.upper_limits)
        {
            *target = target.clamp(*lower, *upper);
        }
        Ok(joint_target)
    }
}

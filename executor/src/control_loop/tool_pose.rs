//! Tool feedback shared by all motion paths, in the station world frame.

use crate::config::ExecutorConfig;
use crate::plant::Feedback;
use crate::robot::Robot;
use anyhow::Context;
use nalgebra::Isometry3;
use std::sync::Arc;

/// The tool's pose from joint readings, for the arm's records, in the station's
/// world frame, which targets are given in.
pub(crate) struct ToolPose {
    robot: Arc<Robot>,
    link: String,
    /// Where the URDF root sits in the world.
    base: Isometry3<f64>,
}

impl ToolPose {
    pub(crate) fn new(
        config: &ExecutorConfig,
        robot: Option<&Arc<Robot>>,
    ) -> anyhow::Result<Option<Self>> {
        let Some(link) = &config.tool_link else {
            return Ok(None);
        };
        Ok(Some(Self {
            robot: Arc::clone(robot.context("A tool link needs the robot model")?),
            link: link.clone(),
            base: config
                .base_pose
                .as_ref()
                .map_or_else(Isometry3::identity, crate::config::BasePose::isometry),
        }))
    }

    /// Position and quaternion (xyzw); none where the model cannot say.
    pub(crate) fn pose(&self, feedback: &Feedback) -> Option<([f64; 3], [f64; 4])> {
        let pose = self.base
            * self
                .robot
                .pose_at(&self.link, &feedback.positions, feedback.gripper_angle)
                .ok()?;
        let translation = pose.translation.vector;
        let rotation = pose.rotation;
        Some((
            [translation.x, translation.y, translation.z],
            [rotation.i, rotation.j, rotation.k, rotation.w],
        ))
    }
}

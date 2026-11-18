use crate::numeric;
use hebbian_robot_geometry::robot::Robot as GeometryRobot;
use std::ops::Deref;
use std::path::Path;
use std::sync::Arc;

pub struct Robot {
    geometry: Arc<GeometryRobot>,
}

impl Robot {
    pub fn load(urdf: &Path, driven_joints: &[String]) -> anyhow::Result<Self> {
        Ok(Self {
            geometry: Arc::new(GeometryRobot::load(urdf, driven_joints)?),
        })
    }

    pub(crate) fn geometry(&self) -> Arc<GeometryRobot> {
        Arc::clone(&self.geometry)
    }

    // CONSTRAINT: The extra driven joint is the gripper; a missing jaw angle is treated as closed.
    #[must_use]
    pub(crate) fn driven_values(&self, arm: &[f64], gripper_angle: Option<f64>) -> Vec<f64> {
        let mut joints = arm.to_vec();
        if self.geometry.driven_joint_count() == arm.len() + 1 {
            joints.push(gripper_angle.unwrap_or(0.0));
        }
        joints
    }

    pub(crate) fn pose_at(
        &self,
        link: &str,
        arm: &[f64],
        gripper_angle: Option<f64>,
    ) -> anyhow::Result<nalgebra::Isometry3<f64>> {
        self.link_pose(link, &self.driven_values(arm, gripper_angle))
    }
}

impl Deref for Robot {
    type Target = GeometryRobot;
    fn deref(&self) -> &Self::Target {
        &self.geometry
    }
}

const PATH_STEP_RADIANS: f64 = 0.02;

pub(crate) fn path_samples(start: &[f64], goal: &[f64]) -> impl Iterator<Item = Vec<f64>> {
    let largest = start
        .iter()
        .zip(goal)
        .map(|(a, b)| (b - a).abs())
        .fold(0.0, f64::max);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let samples = ((largest / PATH_STEP_RADIANS).ceil() as usize).max(1);
    (1..=samples).map(move |index| {
        let fraction = numeric::count(index) / numeric::count(samples);
        start
            .iter()
            .zip(goal)
            .map(|(a, b)| a + (b - a) * fraction)
            .collect()
    })
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum PathCheck {
    Clear,
    StartRefused { reason: String },
    StopsAt { last_safe: Vec<f64>, reason: String },
}

impl PathCheck {
    pub(crate) fn refusal(self) -> Option<String> {
        match self {
            Self::Clear => None,
            Self::StartRefused { reason } | Self::StopsAt { reason, .. } => Some(reason),
        }
    }
}

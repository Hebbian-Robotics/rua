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
        let has_gripper = self.geometry.driven_joint_count() == arm.len() + 1;
        let mut joints = Vec::with_capacity(arm.len() + usize::from(has_gripper));
        joints.extend_from_slice(arm);
        if has_gripper {
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

pub(crate) fn path_sample_fractions(start: &[f64], goal: &[f64]) -> impl Iterator<Item = f64> {
    let largest = start
        .iter()
        .zip(goal)
        .map(|(a, b)| (b - a).abs())
        .fold(0.0, f64::max);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let samples = ((largest / PATH_STEP_RADIANS).ceil() as usize).max(1);
    (1..=samples).map(move |index| numeric::count(index) / numeric::count(samples))
}

pub(crate) fn interpolate_path_sample(
    start: &[f64],
    goal: &[f64],
    fraction: f64,
    output: &mut [f64],
) {
    for ((start, goal), joint) in start.iter().zip(goal).zip(output) {
        *joint = start + (goal - start) * fraction;
    }
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

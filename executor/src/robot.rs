//! Geometry exports and executor-owned path sampling policy.
use crate::numeric;
use hebbian_robot_geometry::robot::Robot as GeometryRobot;
use std::ops::Deref;
use std::path::Path;
use std::sync::Arc;

/// Geometry with RUA's arm/gripper slot convention at the host boundary.
pub struct Robot {
    geometry: Arc<GeometryRobot>,
}

impl Robot {
    /// Read the model for the configured driven joints.
    ///
    /// # Errors
    /// The model or its driven joints are invalid.
    pub fn load(urdf: &Path, driven_joints: &[String]) -> anyhow::Result<Self> {
        Ok(Self {
            geometry: Arc::new(GeometryRobot::load(urdf, driven_joints)?),
        })
    }

    pub(crate) fn geometry(&self) -> Arc<GeometryRobot> {
        Arc::clone(&self.geometry)
    }

    /// The values for every driven joint, from the arm's and the jaw's angle:
    /// when the URDF drives one joint more than the arm has, it is the gripper's,
    /// and a jaw whose angle is unknown counts as closed.
    #[must_use]
    pub(crate) fn driven_values(&self, arm: &[f64], gripper_angle: Option<f64>) -> Vec<f64> {
        let mut joints = arm.to_vec();
        if self.geometry.driven_joint_count() == arm.len() + 1 {
            joints.push(gripper_angle.unwrap_or(0.0));
        }
        joints
    }

    /// The pose of a link in the root frame, at the arm's joints and the jaw's
    /// angle.
    ///
    /// # Errors
    /// The joint values do not match the driven joints, or the link is unknown.
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

/// The straight joint path between two states is sampled this finely.
const PATH_STEP_RADIANS: f64 = 0.02;

/// The states along the straight joint path from `start` to `goal`, after the
/// start and ending at the goal, at most `PATH_STEP_RADIANS` apart in any joint.
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

/// How far along a straight joint path a check lets the arm go.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum PathCheck {
    /// Every sampled step passes.
    Clear,
    /// The start itself is refused, so no part of the path is allowed.
    StartRefused { reason: String },
    /// The path passes up to `last_safe`, which is the start when the first
    /// step is refused.
    StopsAt { last_safe: Vec<f64>, reason: String },
}

impl PathCheck {
    /// Why the path does not pass whole, if it does not.
    pub(crate) fn refusal(self) -> Option<String> {
        match self {
            Self::Clear => None,
            Self::StartRefused { reason } | Self::StopsAt { reason, .. } => Some(reason),
        }
    }
}

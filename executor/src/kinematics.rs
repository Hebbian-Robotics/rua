//! Serial-chain geometry shared by URDF loading and inverse kinematics.
//!
//! Computes poses and Jacobians from joint angles without station profiles,
//! command admission or safety policy. The solver supplies its reach tolerance.

use anyhow::ensure;
use nalgebra::{DMatrix, Isometry3, Quaternion, Translation3, Unit, UnitQuaternion, Vector3};

/// One movable joint of the chain: where its frame sits relative to the
/// previous joint's frame (fixed joints between them folded in), and its axis.
#[derive(Debug, Clone)]
pub(crate) struct ChainJoint {
    pub(crate) before: Isometry3<f64>,
    pub(crate) axis: Unit<Vector3<f64>>,
}

/// The arm from the URDF root to the tool: revolute joints in driven order.
#[derive(Debug, Clone)]
pub struct SerialChain {
    joints: Vec<ChainJoint>,
    tool_offset: Isometry3<f64>,
}

/// Each joint's frame (before its rotation) in the root frame, and the tool's.
pub(crate) struct Frames {
    joints: Vec<Isometry3<f64>>,
    pub(crate) tool: Isometry3<f64>,
}

impl SerialChain {
    pub(crate) fn new(joints: Vec<ChainJoint>, tool_offset: Isometry3<f64>) -> Self {
        Self {
            joints,
            tool_offset,
        }
    }

    /// The same chain with its root placed at `base`, so poses are in the frame
    /// `base` is given in.
    #[must_use]
    pub fn placed_at(mut self, base: Isometry3<f64>) -> Self {
        match self.joints.first_mut() {
            Some(first) => first.before = base * first.before,
            None => self.tool_offset = base * self.tool_offset,
        }
        self
    }

    #[must_use]
    pub fn joint_count(&self) -> usize {
        self.joints.len()
    }

    /// Whether `position` is farther from the first joint than the links after
    /// it reach laid end to end, so no joints put the tool there.
    pub(crate) fn out_of_reach(&self, position: &Vector3<f64>, tolerance_meters: f64) -> bool {
        let Some((first, rest)) = self.joints.split_first() else {
            return true;
        };
        let reach = rest
            .iter()
            .map(|joint| joint.before.translation.vector.norm())
            .sum::<f64>()
            + self.tool_offset.translation.vector.norm();
        (position - first.before.translation.vector).norm() > reach + tolerance_meters
    }

    pub(crate) fn frames(&self, joints: &[f64]) -> Frames {
        let mut pose = Isometry3::identity();
        let mut frames = Vec::with_capacity(self.joints.len());
        for (joint, angle) in self.joints.iter().zip(joints.iter()) {
            pose *= joint.before;
            frames.push(pose);
            pose *= Isometry3::from_parts(
                Translation3::identity(),
                UnitQuaternion::from_axis_angle(&joint.axis, *angle),
            );
        }
        Frames {
            joints: frames,
            tool: pose * self.tool_offset,
        }
    }

    /// The tool's pose in the root frame.
    #[must_use]
    pub fn tool_pose(&self, joints: &[f64]) -> Isometry3<f64> {
        self.frames(joints).tool
    }

    /// Each joint's origin, then the tool's, in the root frame: the points a
    /// viewer draws the arm through.
    #[must_use]
    pub fn model_points(&self, joints: &[f64]) -> Vec<[f64; 3]> {
        let frames = self.frames(joints);
        frames
            .joints
            .iter()
            .chain(std::iter::once(&frames.tool))
            .map(|frame| frame.translation.vector.into())
            .collect()
    }

    /// The geometric Jacobian of the tool: three rows of linear velocity and
    /// three of angular velocity, in the root frame, one column per joint.
    pub(crate) fn jacobian(&self, frames: &Frames) -> DMatrix<f64> {
        let tool = frames.tool.translation.vector;
        let mut jacobian = DMatrix::zeros(6, self.joints.len());
        for (column, (joint, frame)) in self.joints.iter().zip(&frames.joints).enumerate() {
            let axis = frame.rotation * joint.axis.into_inner();
            let linear = axis.cross(&(tool - frame.translation.vector));
            for row in 0..3 {
                jacobian[(row, column)] = linear[row];
                jacobian[(row + 3, column)] = axis[row];
            }
        }
        jacobian
    }
}

/// A station-frame pose from a position and an xyzw quaternion.
///
/// # Errors
/// The quaternion is not a unit quaternion.
pub fn pose_from_xyzw(position: [f64; 3], quaternion: [f64; 4]) -> anyhow::Result<Isometry3<f64>> {
    let squared = quaternion.iter().map(|value| value * value).sum::<f64>();
    ensure!(
        (squared.sqrt() - 1.0).abs() < 1e-5,
        "The target quaternion must be a unit quaternion"
    );
    Ok(Isometry3::from_parts(
        Translation3::from(position),
        UnitQuaternion::from_quaternion(Quaternion::new(
            quaternion[3],
            quaternion[0],
            quaternion[1],
            quaternion[2],
        )),
    ))
}

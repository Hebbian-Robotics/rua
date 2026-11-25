use anyhow::ensure;
use nalgebra::{DMatrix, Isometry3, Quaternion, Translation3, Unit, UnitQuaternion, Vector3};

// CONSTRAINT: Joint origins include intervening fixed joints; axes are expressed in the joint
// frame supplied by the URDF.
#[derive(Debug, Clone)]
pub(crate) struct ChainJoint {
    pub(crate) before: Isometry3<f64>,
    pub(crate) axis: Unit<Vector3<f64>>,
}

#[derive(Debug, Clone)]
pub struct SerialChain {
    joints: Vec<ChainJoint>,
    tool_offset: Isometry3<f64>,
}

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

    fn evaluate(
        &self,
        joints: &[f64],
        mut visit_joint: impl FnMut(&Isometry3<f64>),
    ) -> Isometry3<f64> {
        let mut pose = Isometry3::identity();
        for (joint, angle) in self.joints.iter().zip(joints.iter()) {
            pose *= joint.before;
            visit_joint(&pose);
            pose *= Isometry3::from_parts(
                Translation3::identity(),
                UnitQuaternion::from_axis_angle(&joint.axis, *angle),
            );
        }
        pose * self.tool_offset
    }

    pub(crate) fn frames(&self, joints: &[f64]) -> Frames {
        let mut frames = Vec::with_capacity(self.joints.len());
        let tool = self.evaluate(joints, |pose| frames.push(*pose));
        Frames {
            joints: frames,
            tool,
        }
    }

    #[must_use]
    pub fn tool_pose(&self, joints: &[f64]) -> Isometry3<f64> {
        self.evaluate(joints, |_| {})
    }

    #[must_use]
    pub fn model_points(&self, joints: &[f64]) -> Vec<[f64; 3]> {
        self.tool_pose_and_model_points(joints).1
    }

    pub(crate) fn tool_pose_and_model_points(
        &self,
        joints: &[f64],
    ) -> (Isometry3<f64>, Vec<[f64; 3]>) {
        let mut points = Vec::with_capacity(self.joints.len() + 1);
        let tool = self.evaluate(joints, |pose| points.push(pose.translation.vector.into()));
        points.push(tool.translation.vector.into());
        (tool, points)
    }

    // CONSTRAINT: Jacobian rows are linear then angular velocity, expressed in the root frame.
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

// WHY: Four components rounded to four decimals introduce at most 0.0001 norm error.
pub const UNIT_QUATERNION_TOLERANCE: f64 = 0.00011;

pub fn pose_from_xyzw(position: [f64; 3], quaternion: [f64; 4]) -> anyhow::Result<Isometry3<f64>> {
    pose_with_tolerance(position, quaternion, UNIT_QUATERNION_TOLERANCE)
}

pub fn stream_pose_from_xyzw(
    position: [f64; 3],
    quaternion: [f64; 4],
) -> anyhow::Result<Isometry3<f64>> {
    pose_with_tolerance(position, quaternion, 1e-5)
}

fn pose_with_tolerance(
    position: [f64; 3],
    quaternion: [f64; 4],
    tolerance: f64,
) -> anyhow::Result<Isometry3<f64>> {
    ensure!(
        position.iter().all(|value| value.is_finite()),
        "A pose goal needs a finite position"
    );
    let squared = quaternion.iter().map(|value| value * value).sum::<f64>();
    ensure!(
        (squared.sqrt() - 1.0).abs() <= tolerance,
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

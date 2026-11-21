pub use hebbian_robot_geometry::kinematics::{SerialChain, pose_from_xyzw};

pub(crate) fn base_pose_isometry(base: &crate::config::BasePose) -> nalgebra::Isometry3<f64> {
    nalgebra::Isometry3::new(
        nalgebra::Vector3::from(base.position_meters),
        nalgebra::Vector3::z() * base.yaw_radians,
    )
}

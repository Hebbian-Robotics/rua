use crate::{config::ControllerConfig, robot::Robot};
pub use hebbian_robot_geometry::ik::{
    IkRefusal, IkSolver, ORIENTATION_TOLERANCE_RADIANS, POSITION_TOLERANCE_METERS,
};

pub(crate) fn solver_for_arm(
    config: &ControllerConfig,
    robot: &Robot,
) -> anyhow::Result<Option<IkSolver>> {
    let Some(tool_link) = &config.tool_link else {
        return Ok(None);
    };
    let mut chain = robot.serial_chain(tool_link, config.joint_count)?;
    if let Some(base) = &config.base_pose {
        chain = chain.placed_at(crate::kinematics::base_pose_isometry(base));
    }
    IkSolver::new(
        chain,
        &config.lower_limits,
        &config.upper_limits,
        config.joint_limit_recovery_tolerance_radians,
    )
    .map(Some)
}

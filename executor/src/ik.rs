//! RUA configuration adapter for the shared inverse-kinematics solver.
use crate::{config::ExecutorConfig, robot::Robot};
pub use hebbian_robot_geometry::ik::{
    IkRefusal, IkSolver, ORIENTATION_TOLERANCE_RADIANS, POSITION_TOLERANCE_METERS,
};

/// The solver for the arm a config describes: `None` when it names no tool
/// link to solve for.
///
/// # Errors
/// The URDF cannot be read, or the tool is not at the end of the arm's joints.
pub(crate) fn solver_for_arm(
    config: &ExecutorConfig,
    robot: &Robot,
) -> anyhow::Result<Option<IkSolver>> {
    let Some(tool_link) = &config.tool_link else {
        return Ok(None);
    };
    let mut chain = robot.serial_chain(tool_link, config.joint_count)?;
    if let Some(base) = &config.base_pose {
        chain = chain.placed_at(base.isometry());
    }
    IkSolver::new(
        chain,
        &config.lower_limits,
        &config.upper_limits,
        config.joint_limit_recovery_tolerance_radians,
    )
    .map(Some)
}

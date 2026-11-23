use std::path::Path;
use std::sync::Arc;

use anyhow::Context;

use crate::config::ControllerConfig;
use crate::limits::Trip;
use crate::plant::Feedback;
use crate::robot::Robot;
use crate::self_collision::SelfCollisionGuard;
use crate::surface_guard::SurfaceGuard;

// SAFETY: The servo setpoint clamped into the joint limits: the one place a setpoint's
// limits are applied, at the write, whatever produced it (a hold at a measured
// pose that overshot a limit, a filter braking past one). A setpoint that was
// already outside, as after enabling just past a limit, may stay there or move
// inward, never farther out.
pub(crate) fn clamp_setpoint(config: &ControllerConfig, command: &mut [f64], previous: &[f64]) {
    for (joint, value) in command.iter_mut().enumerate() {
        let lower = config.lower_limits[joint].min(previous[joint]);
        let upper = config.upper_limits[joint].max(previous[joint]);
        *value = value.clamp(lower, upper);
    }
}

pub(crate) fn guard_command(
    config: &ControllerConfig,
    guard: Option<&SurfaceGuard>,
    self_collision: Option<&SelfCollisionGuard>,
    command: &[f64],
    previous_command: &[f64],
    feedback: &Feedback,
) -> Result<(), Trip> {
    if !crate::limits::is_finite_joint_vector(command, config.joint_count)
        || !crate::limits::is_finite_joint_vector(previous_command, config.joint_count)
        || !crate::limits::is_finite_joint_vector(&feedback.positions, config.joint_count)
    {
        return Err(Trip::InvalidSetpoint);
    }
    if let Some(guard) = guard {
        let reason = guard
            .path_reason(&feedback.positions, command, feedback.gripper_angle)
            .map_err(|error| Trip::SurfaceGuard(error.to_string()))?;
        if let Some(reason) = reason {
            return Err(Trip::SurfaceGuard(reason));
        }
    }
    if let Some(self_collision) = self_collision {
        let reason = self_collision
            .path_reason(&feedback.positions, command, feedback.gripper_angle)
            .map_err(|error| Trip::SelfCollision(error.to_string()))?;
        if let Some(reason) = reason {
            return Err(Trip::SelfCollision(reason));
        }
    }
    Ok(())
}

pub(crate) fn load_shared_robot(config: &ControllerConfig) -> anyhow::Result<Option<Arc<Robot>>> {
    let needed = config.surface_guard.is_some()
        || config.self_collision.is_some()
        || config.policy.is_some()
        || config.tool_link.is_some();
    if !needed {
        return Ok(None);
    }
    let mut robot = Robot::load(Path::new(&config.urdf), &config.driven_joints)?;
    if let Some(gripper) = &config.gripper {
        robot = robot.with_closed_gripper_angle(gripper.closed_radians);
    }
    Ok(Some(Arc::new(robot)))
}

pub(crate) fn build_self_collision(
    config: &ControllerConfig,
    robot: Option<&Arc<Robot>>,
) -> anyhow::Result<Option<SelfCollisionGuard>> {
    let Some(settings) = &config.self_collision else {
        return Ok(None);
    };
    let robot = robot.context("The self-collision check needs the robot model")?;
    SelfCollisionGuard::new(
        Arc::clone(robot),
        settings.penetration_tolerance_meters,
        &settings.allowed_collision_pairs,
    )
    .map(Some)
}

pub(crate) fn build_guard(
    config: &ControllerConfig,
    robot: Option<&Arc<Robot>>,
) -> anyhow::Result<Option<SurfaceGuard>> {
    let Some(settings) = &config.surface_guard else {
        return Ok(None);
    };
    let robot = robot.context("The table guard needs the robot model")?;
    Ok(Some(SurfaceGuard::new(settings, Arc::clone(robot))))
}

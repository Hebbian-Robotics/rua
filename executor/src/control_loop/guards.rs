//! The checks every servo setpoint passes before it is sent.

use std::path::Path;
use std::sync::Arc;

use anyhow::Context;

use crate::config::ExecutorConfig;
use crate::limits::Trip;
use crate::plant::Feedback;
use crate::robot::Robot;
use crate::self_collision::SelfCollisionGuard;
use crate::surface_guard::SurfaceGuard;

/// The servo setpoint clamped into the joint limits: the one place a setpoint's
/// limits are applied, at the write, whatever produced it (a hold at a measured
/// pose that overshot a limit, a filter braking past one). A setpoint that was
/// already outside, as after enabling just past a limit, may stay there or move
/// inward, never farther out.
pub(crate) fn clamp_setpoint(config: &ExecutorConfig, command: &mut [f64], previous: &[f64]) {
    for (joint, value) in command.iter_mut().enumerate() {
        let lower = config.lower_limits[joint].min(previous[joint]);
        let upper = config.upper_limits[joint].max(previous[joint]);
        *value = value.clamp(lower, upper);
    }
}

/// Validate a ready servo setpoint and its path; it is sent as it is. Planned trajectories and policy
/// actions arrive directly; changing destinations are smoothed before this boundary.
pub(crate) fn guard_command(
    config: &ExecutorConfig,
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
        return Err(Trip("Invalid servo setpoint".into()));
    }
    if let Some(guard) = guard {
        let reason = guard
            .path_reason(&feedback.positions, command, feedback.gripper_angle)
            .map_err(|error| Trip(error.to_string()))?;
        if let Some(reason) = reason {
            return Err(Trip(reason));
        }
    }
    if let Some(self_collision) = self_collision {
        let reason = self_collision
            .path_reason(&feedback.positions, command, feedback.gripper_angle)
            .map_err(|error| Trip(error.to_string()))?;
        if let Some(reason) = reason {
            return Err(Trip(reason));
        }
    }
    Ok(())
}

/// The robot model, loaded once for everything on this arm that reads it: the
/// guards, the policy and the tool pose. None when none of them is configured.
///
/// # Errors
/// The URDF cannot be loaded.
pub(crate) fn load_shared_robot(config: &ExecutorConfig) -> anyhow::Result<Option<Arc<Robot>>> {
    let needed = config.surface_guard.is_some()
        || config.self_collision.is_some()
        || config.policy.is_some()
        || config.tool_link.is_some();
    if !needed {
        return Ok(None);
    }
    Ok(Some(Arc::new(Robot::load(
        Path::new(&config.urdf),
        &config.driven_joints,
    )?)))
}

pub(crate) fn build_self_collision(
    config: &ExecutorConfig,
    robot: Option<&Arc<Robot>>,
) -> anyhow::Result<Option<SelfCollisionGuard>> {
    let Some(settings) = &config.self_collision else {
        return Ok(None);
    };
    let robot = robot.context("The self-collision check needs the robot model")?;
    SelfCollisionGuard::new(Arc::clone(robot), settings.penetration_tolerance_meters).map(Some)
}

pub(crate) fn build_guard(
    config: &ExecutorConfig,
    robot: Option<&Arc<Robot>>,
) -> anyhow::Result<Option<SurfaceGuard>> {
    let Some(settings) = &config.surface_guard else {
        return Ok(None);
    };
    let robot = robot.context("The table guard needs the robot model")?;
    Ok(Some(SurfaceGuard::new(settings, Arc::clone(robot))))
}

//! Keep the arm from being driven down through the table it stands on.
//!
//! Light contact with the surface is fine, but a step that takes any part of the
//! arm further below it is refused. Only descent is refused, so an arm already
//! past the surface can still climb out.

use std::sync::Arc;

use crate::robot::{PathCheck, Robot, path_samples};
use hebbian_contracts::SurfaceGuardConfig;

/// A descent smaller than this is numerical noise, not driving into the table.
const DESCENT_NOISE_METERS: f64 = 1e-6;
pub(crate) struct SurfaceGuard {
    floor: f64,
    robot: Arc<Robot>,
}

impl SurfaceGuard {
    #[must_use]
    pub fn new(settings: &SurfaceGuardConfig, robot: Arc<Robot>) -> Self {
        Self {
            floor: settings.surface_height_meters - settings.leeway_meters,
            robot,
        }
    }

    /// Height of the lowest moving part, in the arm's base frame.
    ///
    /// # Errors
    /// The joint values are malformed.
    pub fn lowest_point(&self, arm: &[f64], gripper_angle: Option<f64>) -> anyhow::Result<f64> {
        self.robot
            .lowest_point(&self.robot.driven_values(arm, gripper_angle))
    }

    /// Why going from one lowest point to another is refused, if it is.
    #[must_use]
    pub(crate) fn descent_reason(&self, before: f64, after: f64) -> Option<String> {
        (after < self.floor && after < before - DESCENT_NOISE_METERS).then(|| {
            format!(
                "The arm would go {:.1} mm past the table surface guard's limit; raise the target",
                (self.floor - after) * 1000.0
            )
        })
    }

    /// Check jaw motion at the current arm pose, including recovery from below the table.
    pub(crate) fn jaw_descent_reason(
        &self,
        arm: &[f64],
        previous_angle: f64,
        next_angle: f64,
    ) -> anyhow::Result<Option<String>> {
        let before = self.lowest_point(arm, Some(previous_angle))?;
        let after = self.lowest_point(arm, Some(next_angle))?;
        Ok(self.descent_reason(before, after))
    }

    /// Why the straight joint path from `start` to `goal` is refused, if it is:
    /// the per-cycle trip's reading of [`Self::safe_prefix`], which the planner
    /// and the preview cut paths short with.
    ///
    /// # Errors
    /// The joint values are malformed.
    pub(crate) fn path_reason(
        &self,
        start: &[f64],
        goal: &[f64],
        gripper_angle: Option<f64>,
    ) -> anyhow::Result<Option<String>> {
        Ok(self.safe_prefix(start, goal, gripper_angle)?.refusal())
    }

    /// How far along the straight joint path from `start` to `goal` the arm may
    /// go: up to the last sampled step before the first that descends past the
    /// limit. The start itself is never refused, so an arm can climb out.
    ///
    /// # Errors
    /// The joint values are malformed.
    pub(crate) fn safe_prefix(
        &self,
        start: &[f64],
        goal: &[f64],
        gripper_angle: Option<f64>,
    ) -> anyhow::Result<PathCheck> {
        let mut previous = self.lowest_point(start, gripper_angle)?;
        if start == goal {
            return Ok(PathCheck::Clear);
        }
        let mut last_safe = start.to_vec();
        for candidate in path_samples(start, goal) {
            let lowest = self.lowest_point(&candidate, gripper_angle)?;
            if let Some(reason) = self.descent_reason(previous, lowest) {
                return Ok(PathCheck::StopsAt { last_safe, reason });
            }
            previous = lowest;
            last_safe = candidate;
        }
        Ok(PathCheck::Clear)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A 0.4 m bar, 0.1 m square, pivoting about the world's y axis: at angle 0 its
    /// underside is 0.05 m below the pivot, and its tip dips as the angle grows.
    const BAR_URDF: &str = r#"<robot name="bar">
      <link name="world"/>
      <link name="bar"><collision><origin xyz="0.2 0 0"/><geometry><box size="0.4 0.1 0.1"/></geometry></collision></link>
      <joint name="lift" type="revolute"><parent link="world"/><child link="bar"/>
        <axis xyz="0 1 0"/><limit lower="-3" upper="3" effort="1" velocity="1"/></joint>
    </robot>"#;

    fn guard(name: &str) -> SurfaceGuard {
        let path: PathBuf =
            std::env::temp_dir().join(format!("{name}-{}.urdf", std::process::id()));
        std::fs::write(&path, BAR_URDF).unwrap();
        let robot = Robot::load(&path, &["lift".to_owned()]).unwrap();
        let _ = std::fs::remove_file(path);
        SurfaceGuard::new(
            &SurfaceGuardConfig {
                surface_height_meters: -0.05,
                leeway_meters: 0.01,
            },
            Arc::new(robot),
        )
    }

    #[test]
    fn the_lowest_point_is_the_bars_lowest_corner() {
        let guard = guard("lowest");
        assert!((guard.lowest_point(&[0.0], None).unwrap() + 0.05).abs() < 1e-9);
        let angle: f64 = 0.5;
        let expected = -0.4 * angle.sin() - 0.05 * angle.cos();
        assert!((guard.lowest_point(&[angle], None).unwrap() - expected).abs() < 1e-9);
    }

    #[test]
    fn touching_and_pressing_within_the_leeway_is_allowed_but_not_past_it() {
        let guard = guard("leeway");
        // The floor is 0.06 m below the pivot's plane: the bar at rest is 0.01 m above it.
        assert!(guard.path_reason(&[0.0], &[0.01], None).unwrap().is_none());
        assert!(guard.path_reason(&[0.0], &[0.5], None).unwrap().is_some());
    }

    #[test]
    fn an_arm_already_past_the_limit_can_climb_out_but_not_sink() {
        let guard = guard("climb");
        assert!(guard.path_reason(&[0.5], &[0.2], None).unwrap().is_none());
        assert!(guard.path_reason(&[0.5], &[0.6], None).unwrap().is_some());
    }
    #[test]
    fn jaw_motion_cannot_descend_through_the_table_but_can_recover() {
        let path = std::env::temp_dir().join(format!("jaw-surface-{}.urdf", std::process::id()));
        std::fs::write(&path, BAR_URDF).unwrap();
        // The only driven slot is the gripper; there are no arm joints.
        let robot = Arc::new(Robot::load(&path, &["lift".to_owned()]).unwrap());
        std::fs::remove_file(path).unwrap();
        let guard = SurfaceGuard::new(
            &SurfaceGuardConfig {
                surface_height_meters: -0.05,
                leeway_meters: 0.01,
            },
            robot,
        );
        assert!(guard.jaw_descent_reason(&[], 0.0, 0.01).unwrap().is_none());
        assert!(guard.jaw_descent_reason(&[], 0.0, 0.5).unwrap().is_some());
        assert!(guard.jaw_descent_reason(&[], 0.5, 0.2).unwrap().is_none());
        assert!(guard.jaw_descent_reason(&[], 0.5, 0.6).unwrap().is_some());
    }
}

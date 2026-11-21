use std::sync::Arc;

use crate::robot::{PathCheck, Robot, interpolate_path_sample, path_sample_fractions};
use hebbian_contracts::SurfaceGuardConfig;

// WHY: Ignore numerical noise when deciding whether the arm is descending.
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

    pub fn lowest_point(&self, arm: &[f64], gripper_angle: Option<f64>) -> anyhow::Result<f64> {
        self.robot
            .lowest_point(&self.robot.driven_values(arm, gripper_angle))
    }

    #[must_use]
    pub(crate) fn descent_reason(&self, before: f64, after: f64) -> Option<String> {
        // SAFETY: Allow upward recovery even when the arm is already below the limit.
        (after < self.floor && after < before - DESCENT_NOISE_METERS).then(|| {
            format!(
                "The arm would go {:.1} mm past the table surface guard's limit; raise the target",
                (self.floor - after) * 1000.0
            )
        })
    }

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

    pub(crate) fn path_reason(
        &self,
        start: &[f64],
        goal: &[f64],
        gripper_angle: Option<f64>,
    ) -> anyhow::Result<Option<String>> {
        Ok(self.safe_prefix(start, goal, gripper_angle)?.refusal())
    }

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
        let joint_count = start.len().min(goal.len());
        let mut candidate = self
            .robot
            .driven_values(&start[..joint_count], gripper_angle);
        let mut last_safe_fraction = None;
        for fraction in path_sample_fractions(start, goal) {
            interpolate_path_sample(start, goal, fraction, &mut candidate[..joint_count]);
            let lowest = self.robot.lowest_point(&candidate)?;
            if let Some(reason) = self.descent_reason(previous, lowest) {
                let last_safe = match last_safe_fraction {
                    Some(fraction) => {
                        candidate.truncate(joint_count);
                        interpolate_path_sample(start, goal, fraction, &mut candidate);
                        candidate
                    }
                    None => start.to_vec(),
                };
                return Ok(PathCheck::StopsAt { last_safe, reason });
            }
            previous = lowest;
            last_safe_fraction = Some(fraction);
        }
        Ok(PathCheck::Clear)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // WHY: This bar's known geometry makes its lowest point predictable as it pivots.
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
        // CONSTRAINT: The floor is 0.06 m below the pivot's plane: the bar at rest is 0.01 m above it.
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
        // CONSTRAINT: The only driven slot is the gripper; there are no arm joints.
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

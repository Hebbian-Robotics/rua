//! Executor collision admission and path sampling, backed by shared geometry.
use crate::robot::{PathCheck, Robot, path_samples};
use hebbian_robot_geometry::self_collision::SelfCollisionModel;
use std::sync::Arc;

pub struct SelfCollisionGuard {
    robot: Arc<Robot>,
    model: SelfCollisionModel,
}

impl SelfCollisionGuard {
    /// Build collision geometry using the configured penetration allowance.
    ///
    /// # Errors
    /// The collision geometry or allowance is invalid.
    pub fn new(robot: Arc<Robot>, penetration_tolerance_meters: f64) -> anyhow::Result<Self> {
        let model = SelfCollisionModel::new(robot.geometry(), penetration_tolerance_meters)?;
        Ok(Self { robot, model })
    }

    /// The first self-collision at the arm and gripper pose, if present.
    ///
    /// # Errors
    /// Joint values are malformed or the geometry query fails.
    pub fn collision(
        &self,
        arm: &[f64],
        gripper_angle: Option<f64>,
    ) -> anyhow::Result<Option<String>> {
        self.model
            .collision(&self.robot.driven_values(arm, gripper_angle))
    }

    /// Why the straight joint path from `start` to `goal` is refused, if it is:
    /// the per-cycle trip's reading of [`Self::safe_prefix`], which the planner
    /// and the preview cut paths short with.
    ///
    /// # Errors
    /// The joint values are malformed, or the geometry query fails.
    pub(crate) fn path_reason(
        &self,
        start: &[f64],
        goal: &[f64],
        gripper_angle: Option<f64>,
    ) -> anyhow::Result<Option<String>> {
        Ok(self.safe_prefix(start, goal, gripper_angle)?.refusal())
    }

    /// How far along the straight joint path from `start` to `goal` the arm may
    /// go: refused outright from a start already past the allowance, else up to
    /// the last sampled step before the first collision.
    ///
    /// # Errors
    /// The joint values are malformed, or the geometry query fails.
    pub(crate) fn safe_prefix(
        &self,
        start: &[f64],
        goal: &[f64],
        gripper_angle: Option<f64>,
    ) -> anyhow::Result<PathCheck> {
        if let Some(reason) = self.collision(start, gripper_angle)? {
            return Ok(PathCheck::StartRefused { reason });
        }
        let mut last_safe = start.to_vec();
        for candidate in path_samples(start, goal) {
            if let Some(reason) = self.collision(&candidate, gripper_angle)? {
                return Ok(PathCheck::StopsAt { last_safe, reason });
            }
            last_safe = candidate;
        }
        Ok(PathCheck::Clear)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Three 0.2 m cubes. `world` is the base; `swinger` hangs off `pivot` and,
    /// at rest, sits right on top of `world` (neighbours, by design); `far`
    /// rides on `swinger` 0.3 m above the pivot, clear of `world` at rest, and
    /// swings down onto it when the pivot turns a quarter turn the other way.
    const CUBES_URDF: &str = r#"<robot name="cubes">
      <link name="world"><collision><geometry><box size="0.2 0.2 0.2"/></geometry></collision></link>
      <link name="swinger"><collision><origin xyz="-0.3 0 0"/><geometry><box size="0.2 0.2 0.2"/></geometry></collision></link>
      <link name="mid"/>
      <joint name="base_mount" type="fixed"><parent link="world"/><child link="mid"/>
        <origin xyz="0.3 0 0"/></joint>
      <joint name="pivot" type="revolute"><parent link="mid"/><child link="swinger"/>
        <axis xyz="0 1 0"/><limit lower="-3" upper="3" effort="1" velocity="1"/></joint>
      <link name="far"><collision><origin xyz="0 0 0.3"/><geometry><box size="0.2 0.2 0.2"/></geometry></collision></link>
      <joint name="far_pivot" type="revolute"><parent link="swinger"/><child link="far"/>
        <axis xyz="0 1 0"/><limit lower="-3" upper="3" effort="1" velocity="1"/></joint>
    </robot>"#;

    fn guard(name: &str, tolerance: f64) -> SelfCollisionGuard {
        let path: PathBuf =
            std::env::temp_dir().join(format!("{name}-{}.urdf", std::process::id()));
        std::fs::write(&path, CUBES_URDF).unwrap();
        let robot = Robot::load(&path, &["pivot".to_owned(), "far_pivot".to_owned()]).unwrap();
        let _ = std::fs::remove_file(path);
        SelfCollisionGuard::new(Arc::new(robot), tolerance).unwrap()
    }

    const QUARTER_TURN_DOWN: f64 = -std::f64::consts::FRAC_PI_2;

    #[test]
    fn neighbouring_links_may_overlap_by_design() {
        // `swinger` sits inside `world` at rest, and nothing is refused.
        let guard = guard("neighbours", 0.0);
        assert!(guard.collision(&[0.0, 0.0], None).unwrap().is_none());
    }

    #[test]
    fn a_distant_link_swinging_into_the_base_is_refused() {
        let guard = guard("swing", 0.0);
        let reason = guard.collision(&[QUARTER_TURN_DOWN, 0.0], None).unwrap();
        assert!(reason.is_some_and(|text| text.contains("far") && text.contains("world")));
    }

    #[test]
    fn a_path_is_refused_when_it_reaches_a_collision_and_an_allowance_forgives_overlap() {
        let strict = guard("strict", 0.0);
        let refused = strict
            .path_reason(&[0.0, 0.0], &[QUARTER_TURN_DOWN, 0.0], None)
            .unwrap();
        assert!(refused.is_some());
        let generous = guard("generous", 5.0);
        let allowed = generous
            .path_reason(&[0.0, 0.0], &[QUARTER_TURN_DOWN, 0.0], None)
            .unwrap();
        assert!(allowed.is_none());
    }

    #[test]
    fn a_start_already_in_collision_is_refused_whatever_the_goal() {
        let guard = guard("start", 0.0);
        let reason = guard
            .path_reason(&[QUARTER_TURN_DOWN, 0.0], &[0.0, 0.0], None)
            .unwrap();
        assert!(reason.is_some());
    }
}

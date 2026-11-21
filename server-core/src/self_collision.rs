use crate::robot::{PathCheck, Robot, interpolate_path_sample, path_sample_fractions};
use hebbian_robot_geometry::self_collision::{CollisionWorkspace, SelfCollisionModel};
use std::sync::Arc;

pub struct SelfCollisionGuard {
    robot: Arc<Robot>,
    model: SelfCollisionModel,
}

impl SelfCollisionGuard {
    pub fn new(
        robot: Arc<Robot>,
        penetration_tolerance_meters: f64,
        allowed_collision_pairs: &[[String; 2]],
    ) -> anyhow::Result<Self> {
        let model = SelfCollisionModel::new(robot.geometry(), penetration_tolerance_meters)?
            .with_allowed_collisions(allowed_collision_pairs)?;
        Ok(Self { robot, model })
    }

    pub fn collision(
        &self,
        arm: &[f64],
        gripper_angle: Option<f64>,
    ) -> anyhow::Result<Option<String>> {
        let joints = self.robot.driven_values(arm, gripper_angle);
        self.model.collision(&joints)
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
        let mut workspace = CollisionWorkspace::default();
        let mut candidate = self.robot.driven_values(start, gripper_angle);
        if let Some(reason) = self
            .model
            .collision_with_workspace(&candidate, &mut workspace)?
        {
            return Ok(PathCheck::StartRefused { reason });
        }
        if start == goal {
            return Ok(PathCheck::Clear);
        }
        let joint_count = start.len().min(goal.len());
        if joint_count != start.len() {
            candidate = self
                .robot
                .driven_values(&start[..joint_count], gripper_angle);
        }
        let mut last_safe_fraction = None;
        for fraction in path_sample_fractions(start, goal) {
            interpolate_path_sample(start, goal, fraction, &mut candidate[..joint_count]);
            if let Some(reason) = self
                .model
                .collision_with_workspace(&candidate, &mut workspace)?
            {
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
            last_safe_fraction = Some(fraction);
        }
        Ok(PathCheck::Clear)
    }
}

pub(crate) struct GroupCollisionArm {
    pub(crate) guard: Arc<SelfCollisionGuard>,
    pub(crate) base: nalgebra::Isometry3<f64>,
    pub(crate) gripper_angle: Option<f64>,
}

impl GroupCollisionArm {
    pub(crate) fn joints_with_gripper(
        &self,
        joints: &[f64],
        gripper_angle: Option<f64>,
    ) -> Vec<f64> {
        self.guard.robot.driven_values(joints, gripper_angle)
    }

    pub(crate) fn has_gripper(&self, arm_joint_count: usize) -> bool {
        self.guard.robot.driven_joint_count() == arm_joint_count + 1
    }

    pub(crate) fn body<'a>(
        &'a self,
        joints: &'a [f64],
    ) -> hebbian_robot_geometry::self_collision::CollisionBody<'a> {
        hebbian_robot_geometry::self_collision::CollisionBody {
            model: &self.guard.model,
            joints,
            base: self.base,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

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
        SelfCollisionGuard::new(Arc::new(robot), tolerance, &[]).unwrap()
    }

    const QUARTER_TURN_DOWN: f64 = -std::f64::consts::FRAC_PI_2;

    #[test]
    fn neighbouring_links_may_overlap_by_design() {
        let guard = guard("neighbours", 0.0);
        assert!(guard.collision(&[0.0, 0.0], None).unwrap().is_none());
        assert_eq!(
            guard.safe_prefix(&[0.0, 0.0], &[0.0, 0.0], None).unwrap(),
            PathCheck::Clear
        );
    }

    #[test]
    fn a_distant_link_swinging_into_the_base_is_refused() {
        let guard = guard("swing", 0.0);
        let reason = guard.collision(&[QUARTER_TURN_DOWN, 0.0], None).unwrap();
        assert!(reason.is_some_and(|text| text.contains("far") && text.contains("world")));
    }

    #[test]
    fn an_allowed_contact_does_not_disable_other_collision_checks() {
        let mut guard = guard("allowed-contact", 0.0);
        guard.model = guard
            .model
            .with_allowed_collisions(&[["world".to_owned(), "swinger".to_owned()]])
            .unwrap();
        assert!(
            guard
                .collision(&[QUARTER_TURN_DOWN, 0.0], None)
                .unwrap()
                .is_some()
        );
        guard.model = guard
            .model
            .with_allowed_collisions(&[["far".to_owned(), "world".to_owned()]])
            .unwrap();
        assert!(
            guard
                .path_reason(&[0.0, 0.0], &[QUARTER_TURN_DOWN, 0.0], None)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn invalid_collision_allowances_fail_instead_of_silently_changing_protection() {
        for pair in [["world", "missing"], ["world", "world"]] {
            let guard = guard("invalid-allowance", 0.0);
            assert!(
                guard
                    .model
                    .with_allowed_collisions(&[pair.map(str::to_owned)])
                    .is_err()
            );
        }
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
        assert!(matches!(
            guard
                .safe_prefix(&[QUARTER_TURN_DOWN, 0.0], &[QUARTER_TURN_DOWN, 0.0], None,)
                .unwrap(),
            PathCheck::StartRefused { .. }
        ));
    }
}

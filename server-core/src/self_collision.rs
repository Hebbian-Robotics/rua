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
        let mut workspace = CollisionWorkspace::default();
        let mut candidate = self.robot.driven_values(start, gripper_angle);
        let fractions = std::iter::once(0.0).chain(path_sample_fractions(start, goal));
        for fraction in fractions {
            interpolate_path_sample(start, goal, fraction, &mut candidate[..start.len()]);
            if let Some(reason) = self
                .model
                .collision_with_workspace(&candidate, &mut workspace)?
            {
                return Ok(Some(reason));
            }
            if start == goal {
                break;
            }
        }
        Ok(None)
    }

    pub(crate) fn safe_prefix(
        &self,
        start: &[f64],
        goal: &[f64],
        gripper_angle: Option<f64>,
    ) -> anyhow::Result<PathCheck> {
        if let Some(reason) = self.collision(start, gripper_angle)? {
            return Ok(PathCheck::StartRefused { reason });
        }
        if start == goal {
            return Ok(PathCheck::Clear);
        }
        anyhow::ensure!(start.len() == goal.len(), "Mismatched joint dimensions");
        let joint_speeds: Vec<_> = start
            .iter()
            .zip(goal)
            .map(|(start, goal)| (goal - start).abs())
            .collect();
        let Some((fraction, reason)) =
            self.swept_prefix(&joint_speeds, gripper_angle, |fraction, candidate| {
                interpolate_path_sample(start, goal, fraction, candidate);
            })?
        else {
            return Ok(PathCheck::Clear);
        };
        let mut last_safe = start.to_vec();
        interpolate_path_sample(start, goal, fraction, &mut last_safe);
        Ok(PathCheck::StopsAt { last_safe, reason })
    }

    pub(crate) fn swept_prefix(
        &self,
        arm_speeds: &[f64],
        gripper_angle: Option<f64>,
        mut sample: impl FnMut(f64, &mut [f64]),
    ) -> anyhow::Result<Option<(f64, String)>> {
        let mut speeds = self.robot.driven_values(arm_speeds, gripper_angle);
        let fixed_jaw = speeds.get_mut(arm_speeds.len()).map(std::mem::take);
        self.model.swept_prefix(&speeds, |fraction, joints| {
            sample(fraction, &mut joints[..arm_speeds.len()]);
            if let Some(angle) = fixed_jaw {
                joints[arm_speeds.len()] = angle;
            }
        })
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
        guard_from_urdf(name, tolerance, CUBES_URDF)
    }

    fn guard_from_urdf(name: &str, tolerance: f64, urdf: &str) -> SelfCollisionGuard {
        let path: PathBuf =
            std::env::temp_dir().join(format!("{name}-{}.urdf", std::process::id()));
        std::fs::write(&path, urdf).unwrap();
        let robot = Robot::load(&path, &["pivot".to_owned(), "far_pivot".to_owned()]).unwrap();
        let _ = std::fs::remove_file(path);
        SelfCollisionGuard::new(Arc::new(robot), tolerance, &[]).unwrap()
    }

    #[test]
    fn swept_prefix_stops_for_a_collision_between_clear_samples() {
        let guard = guard_from_urdf(
            "sweep-between-samples",
            0.0,
            r#"<robot name="narrow">
            <link name="base"><collision><origin xyz="1.0005 0.0005 0"/><geometry><box size="0.01 0.002 0.01"/></geometry></collision></link>
            <link name="arm"/>
            <link name="tip"><collision><origin xyz="1 0 0"/><geometry><box size="0.01 0.002 0.01"/></geometry></collision></link>
            <joint name="pivot" type="continuous"><parent link="base"/><child link="arm"/><axis xyz="0 0 1"/></joint>
            <joint name="far_pivot" type="continuous"><parent link="arm"/><child link="tip"/><axis xyz="0 0 1"/></joint>
            </robot>"#,
        );
        let start = [-0.009, 0.0];
        let goal = [0.009, 0.0];
        assert!(guard.collision(&start, None).unwrap().is_none());
        assert!(guard.collision(&goal, None).unwrap().is_none());
        assert!(guard.collision(&[0.0, 0.0], None).unwrap().is_some());
        let PathCheck::StopsAt { last_safe, reason } =
            guard.safe_prefix(&start, &goal, None).unwrap()
        else {
            panic!("Expected the sweep to stop between the start and goal");
        };
        assert!(start[0] < last_safe[0] && last_safe[0] < 0.0);
        assert!(guard.collision(&last_safe, None).unwrap().is_none());
        assert!(reason.contains("base") && reason.contains("tip"));
        assert_eq!(
            guard.safe_prefix(&goal, &[0.02, 0.0], None).unwrap(),
            PathCheck::Clear
        );
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

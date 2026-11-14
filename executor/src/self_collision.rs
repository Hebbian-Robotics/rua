//! Refuse commands that fold the arm into itself.
//!
//! The check is the convex hull of every collision mesh, for each pair that may
//! touch, with a penetration allowance. A hull fills in a mesh's hollows; that
//! is the price of a check that needs no mesh-accurate query.

use std::sync::Arc;

use anyhow::{Context, bail, ensure};
use nalgebra::{Isometry3, Point3, Vector3};
use parry3d_f64::math::{Pose, Rot3, Vec3};
use parry3d_f64::query::contact;
use parry3d_f64::shape::ConvexPolyhedron;

use crate::robot::{PathCheck, Robot, path_samples};

struct Hull {
    link: usize,
    shape: ConvexPolyhedron,
    /// A sphere in the link's frame that holds the whole hull: two hulls whose
    /// spheres are apart cannot touch, so the exact query is skipped.
    centre: Point3<f64>,
    radius: f64,
}

pub struct SelfCollisionGuard {
    robot: Arc<Robot>,
    hulls: Vec<Hull>,
    /// Indices into `hulls` of every pair that may touch.
    pairs: Vec<(usize, usize)>,
    penetration_tolerance_meters: f64,
}

impl SelfCollisionGuard {
    /// Build the hulls and the pair list: every pair of shapes on different
    /// joints, except neighbours (a joint and the one it hangs from), which touch
    /// by design. That includes the jaw and the fixed jaw it nests into, whose
    /// hulls overlap at every pose.
    ///
    /// # Errors
    /// A collision shape is not a mesh or box, or has no volume.
    pub fn new(robot: Arc<Robot>, penetration_tolerance_meters: f64) -> anyhow::Result<Self> {
        ensure!(
            penetration_tolerance_meters.is_finite() && penetration_tolerance_meters >= 0.0,
            "The collision penetration tolerance must be finite and nonnegative"
        );
        let mut hulls = Vec::new();
        let mut joints = Vec::new();
        for shape in robot.collision_shapes() {
            ensure!(
                shape.exact,
                "Link {} has a cylinder, capsule or sphere collision shape; \
                 the self-collision check only supports meshes and boxes",
                robot.link_name(shape.link)
            );
            let points: Vec<Vec3> = shape
                .points
                .iter()
                .map(|point| Vec3::new(point.x, point.y, point.z))
                .collect();
            let hull = ConvexPolyhedron::from_convex_hull(&points).with_context(|| {
                format!("No convex hull for link {}", robot.link_name(shape.link))
            })?;
            let count = shape.points.len().max(1);
            let centre = Point3::from(
                shape
                    .points
                    .iter()
                    .fold(Vector3::zeros(), |sum, point| sum + point.coords)
                    / crate::numeric::count(count),
            );
            let radius = shape
                .points
                .iter()
                .map(|point| (point - centre).norm())
                .fold(0.0, f64::max);
            hulls.push(Hull {
                link: shape.link,
                shape: hull,
                centre,
                radius,
            });
            joints.push(shape.joint);
        }
        let neighbours = |child: Option<usize>, other: Option<usize>| {
            child.is_some_and(|joint| robot.joint_parent(joint) == other)
        };
        let mut pairs = Vec::new();
        for first in 0..hulls.len() {
            for second in first + 1..hulls.len() {
                let (a, b) = (joints[first], joints[second]);
                if a != b && !neighbours(a, b) && !neighbours(b, a) {
                    pairs.push((first, second));
                }
            }
        }
        if hulls.is_empty() {
            bail!("The URDF has no collision geometry");
        }
        Ok(Self {
            robot,
            hulls,
            pairs,
            penetration_tolerance_meters,
        })
    }

    /// The first pair of links overlapping by more than the allowance.
    ///
    /// # Errors
    /// The joint values are malformed, or the geometry query fails.
    pub fn collision(
        &self,
        arm: &[f64],
        gripper_angle: Option<f64>,
    ) -> anyhow::Result<Option<String>> {
        let joints = self.robot.driven_values(arm, gripper_angle);
        ensure!(
            self.robot.accepts(&joints),
            "Expected finite values for every driven joint"
        );
        let link_poses = self.robot.link_poses(&joints);
        let poses: Vec<Pose> = link_poses.iter().map(parry_pose).collect();
        for &(first, second) in &self.pairs {
            let (a, b) = (&self.hulls[first], &self.hulls[second]);
            let apart = (link_poses[a.link] * a.centre - link_poses[b.link] * b.centre).norm();
            if apart > a.radius + b.radius {
                continue;
            }
            let touching = contact(&poses[a.link], &a.shape, &poses[b.link], &b.shape, 0.0)
                .context("The convex hull query is unsupported")?;
            if touching.is_some_and(|found| found.dist < -self.penetration_tolerance_meters) {
                return Ok(Some(format!(
                    "Self-collision between {} and {}",
                    self.robot.link_name(a.link),
                    self.robot.link_name(b.link)
                )));
            }
        }
        Ok(None)
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

fn parry_pose(pose: &Isometry3<f64>) -> Pose {
    let translation = pose.translation.vector;
    let rotation = pose.rotation.coords;
    Pose::from_parts(
        Vec3::new(translation.x, translation.y, translation.z),
        Rot3::from_xyzw(rotation.x, rotation.y, rotation.z, rotation.w),
    )
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

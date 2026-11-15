//! Refuse commands that fold the arm into itself.
//!
//! The check is the convex hull of every collision mesh, for each pair that may
//! touch, with a penetration allowance. A hull fills in a mesh's hollows; that
//! is the price of a check that needs no mesh-accurate query.

use std::sync::Arc;

use anyhow::{Context, bail, ensure};
use nalgebra::{Isometry3, Point3, Vector3};
use parry3d_f64::math::{Pose, Rot3, Vec3};
use parry3d_f64::query::{DefaultQueryDispatcher, QueryDispatcher};
use parry3d_f64::shape::ConvexPolyhedron;

use crate::robot::Robot;

struct Hull {
    link: usize,
    shape: ConvexPolyhedron,
    /// A sphere in the link's frame that holds the whole hull: two hulls whose
    /// spheres are apart cannot touch, so the exact query is skipped.
    centre: Point3<f64>,
    radius: f64,
}

pub struct SelfCollisionModel {
    robot: Arc<Robot>,
    hulls: Vec<Hull>,
    /// Indices into `hulls` of every pair that may touch.
    pairs: Vec<(usize, usize)>,
    penetration_tolerance_meters: f64,
}

impl SelfCollisionModel {
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
    pub fn collision(&self, joints: &[f64]) -> anyhow::Result<Option<String>> {
        ensure!(
            self.robot.accepts(joints),
            "Expected finite values for every driven joint"
        );
        let link_poses = self.robot.link_poses(joints);
        for &(first, second) in &self.pairs {
            let (a, b) = (&self.hulls[first], &self.hulls[second]);
            let apart = (link_poses[a.link] * a.centre - link_poses[b.link] * b.centre).norm();
            if apart > a.radius + b.radius {
                continue;
            }
            let first_pose = parry_pose(&link_poses[a.link]);
            let second_pose = parry_pose(&link_poses[b.link]);
            // Only signed depth is used; leave contact points in the local frames.
            let touching = DefaultQueryDispatcher
                .contact(&first_pose.inv_mul(&second_pose), &a.shape, &b.shape, 0.0)
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
}

fn parry_pose(pose: &Isometry3<f64>) -> Pose {
    let translation = pose.translation.vector;
    let rotation = pose.rotation.coords;
    Pose::from_parts(
        Vec3::new(translation.x, translation.y, translation.z),
        Rot3::from_xyzw(rotation.x, rotation.y, rotation.z, rotation.w),
    )
}

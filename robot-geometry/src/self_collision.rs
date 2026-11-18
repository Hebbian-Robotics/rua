// CONSTRAINT: Convex hulls fill mesh hollows and permit configured penetration; the guard
// prevents self-driving collisions rather than all physical contact.

use std::sync::Arc;

use anyhow::{Context, bail, ensure};
use nalgebra::{Isometry3, Point3, Vector3};
use parry3d_f64::bounding_volume::{Aabb, BoundingVolume};
use parry3d_f64::math::{Pose, Rot3, Vec3};
use parry3d_f64::query::{DefaultQueryDispatcher, QueryDispatcher};
use parry3d_f64::shape::{ConvexPolyhedron, Shape};

use crate::robot::Robot;

// WHY: Support-point reduction keeps the query cheap but places the hull slightly
// inside the full geometry; collision allowances must account for that approximation.
const HULL_DIRECTIONS: usize = 64;

fn extreme_corners(points: &[Point3<f64>]) -> Vec<Point3<f64>> {
    if points.len() <= HULL_DIRECTIONS {
        return points.to_vec();
    }
    let golden_angle = std::f64::consts::PI * (3.0 - 5.0_f64.sqrt());
    let mut kept = std::collections::BTreeSet::new();
    for index in 0..HULL_DIRECTIONS {
        let height = 1.0
            - 2.0 * (crate::numeric::count(index) + 0.5) / crate::numeric::count(HULL_DIRECTIONS);
        let ring = (1.0 - height * height).sqrt();
        let turn = golden_angle * crate::numeric::count(index);
        let direction = Vector3::new(ring * turn.cos(), ring * turn.sin(), height);
        let furthest = points
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| {
                a.coords
                    .dot(&direction)
                    .total_cmp(&b.coords.dot(&direction))
            })
            .map(|(corner, _)| corner);
        kept.extend(furthest);
    }
    kept.into_iter().map(|corner| points[corner]).collect()
}

struct Hull {
    link: usize,
    shape: ConvexPolyhedron,
    // WHY: Disjoint bounding spheres rule out contact without an expensive exact query.
    centre: Point3<f64>,
    radius: f64,
    bounds: Aabb,
}

pub struct SelfCollisionModel {
    robot: Arc<Robot>,
    hulls: Vec<Hull>,
    pairs: Vec<(usize, usize)>,
    penetration_tolerance_meters: f64,
}

impl SelfCollisionModel {
    pub fn with_allowed_collisions(
        mut self,
        allowed_pairs: &[[String; 2]],
    ) -> anyhow::Result<Self> {
        for [first_link, second_link] in allowed_pairs {
            ensure!(
                first_link != second_link,
                "A collision allowance needs two different links"
            );
            for link in [first_link, second_link] {
                ensure!(
                    self.hulls
                        .iter()
                        .any(|hull| self.robot.link_name(hull.link) == link),
                    "Collision allowance names link {link} without collision geometry"
                );
            }
        }
        self.pairs.retain(|&(first, second)| {
            let first_link = self.robot.link_name(self.hulls[first].link);
            let second_link = self.robot.link_name(self.hulls[second].link);
            !allowed_pairs.iter().any(|[allowed_first, allowed_second]| {
                (first_link == allowed_first && second_link == allowed_second)
                    || (first_link == allowed_second && second_link == allowed_first)
            })
        });
        Ok(self)
    }

    // CONSTRAINT: Neighbouring joints touch by design, including a jaw nested into its fixed jaw;
    // self-collision pairs exclude those adjacent joints.
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
            let as_parry = |points: &[Point3<f64>]| -> Vec<Vec3> {
                points
                    .iter()
                    .map(|point| Vec3::new(point.x, point.y, point.z))
                    .collect()
            };
            let reduced = extreme_corners(&shape.points);
            // CONSTRAINT: A thin part's extreme corners can be coplanar and have no hull;
            // such a part keeps every corner.
            let (corners, hull) = match ConvexPolyhedron::from_convex_hull(&as_parry(&reduced)) {
                Some(hull) => (reduced, hull),
                None => (
                    shape.points.clone(),
                    ConvexPolyhedron::from_convex_hull(&as_parry(&shape.points)).with_context(
                        || format!("No convex hull for link {}", robot.link_name(shape.link)),
                    )?,
                ),
            };
            let count = corners.len().max(1);
            let centre = Point3::from(
                corners
                    .iter()
                    .fold(Vector3::zeros(), |sum, point| sum + point.coords)
                    / crate::numeric::count(count),
            );
            let radius = corners
                .iter()
                .map(|point| (point - centre).norm())
                .fold(0.0, f64::max);
            let bounds = hull.compute_local_aabb();
            hulls.push(Hull {
                link: shape.link,
                shape: hull,
                centre,
                radius,
                bounds,
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

    pub fn collision(&self, joints: &[f64]) -> anyhow::Result<Option<String>> {
        ensure!(
            self.robot.accepts(joints),
            "Expected finite values for every driven joint"
        );
        let link_poses = self.robot.link_poses(joints);
        let hull_poses: Vec<_> = self
            .hulls
            .iter()
            .map(|hull| parry_pose(&link_poses[hull.link]))
            .collect();
        let hull_bounds: Vec<_> = self
            .hulls
            .iter()
            .zip(&hull_poses)
            .map(|(hull, pose)| hull.bounds.transform_by(pose))
            .collect();
        for &(first, second) in &self.pairs {
            let (a, b) = (&self.hulls[first], &self.hulls[second]);
            let apart = (link_poses[a.link] * a.centre - link_poses[b.link] * b.centre).norm();
            if apart > a.radius + b.radius {
                continue;
            }
            // WHY: Spheres overlap readily around thin brackets and fingers. These
            // boxes enclose the full hulls, so disjoint boxes safely avoid the
            // signed-depth query without changing the penetration allowance.
            if !hull_bounds[first].intersects(&hull_bounds[second]) {
                continue;
            }
            let first_pose = &hull_poses[first];
            let second_pose = &hull_poses[second];
            // WHY: Only signed depth is used; leave contact points in the local frames.
            let touching = DefaultQueryDispatcher
                .contact(&first_pose.inv_mul(second_pose), &a.shape, &b.shape, 0.0)
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

#[cfg(test)]
mod tests {
    use super::*;
    use parry3d_f64::query::PointQuery;

    #[test]
    fn a_reduced_hull_stays_close_inside_a_rounded_part() {
        // WHY: A finely tessellated rounded part is the worst case for keeping few corners.
        let radius = 0.05;
        let part: Vec<Point3<f64>> = (0..4000)
            .map(|index| {
                let height = 1.0 - 2.0 * (f64::from(index) + 0.5) / 4000.0;
                let ring = (1.0 - height * height).sqrt();
                let turn = 2.4 * f64::from(index);
                Point3::new(
                    radius * ring * turn.cos(),
                    radius * ring * turn.sin(),
                    0.5 * radius * height,
                )
            })
            .collect();
        let corners = extreme_corners(&part);
        assert!(corners.len() <= HULL_DIRECTIONS);
        let as_parry: Vec<Vec3> = corners
            .iter()
            .map(|point| Vec3::new(point.x, point.y, point.z))
            .collect();
        let hull = ConvexPolyhedron::from_convex_hull(&as_parry).unwrap();
        let furthest_outside = part
            .iter()
            .map(|point| hull.distance_to_local_point(Vec3::new(point.x, point.y, point.z), true))
            .fold(0.0, f64::max);
        assert!(furthest_outside < 0.005, "{furthest_outside}");
    }
}

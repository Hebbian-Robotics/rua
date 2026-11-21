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
        self.collision_with_workspace(joints, &mut CollisionWorkspace::default())
    }

    pub fn collision_with_workspace(
        &self,
        joints: &[f64],
        workspace: &mut CollisionWorkspace,
    ) -> anyhow::Result<Option<String>> {
        Ok(workspace
            .scene_collision(
                &[CollisionBody {
                    model: self,
                    joints,
                    base: Isometry3::identity(),
                }],
                0.0,
            )?
            .map(|hit| {
                format!(
                    "Self-collision between {} and {}",
                    hit.links[0], hit.links[1]
                )
            }))
    }
}

pub struct CollisionBody<'a> {
    pub model: &'a SelfCollisionModel,
    pub joints: &'a [f64],
    pub base: Isometry3<f64>,
}

pub struct CollisionHit<'a> {
    pub bodies: [usize; 2],
    pub links: [&'a str; 2],
}

struct HullPlacement {
    pose: Pose,
    bounds: Aabb,
    centre: Point3<f64>,
}

impl CollisionBody<'_> {
    fn place_hulls(&self, placements: &mut Vec<HullPlacement>) -> anyhow::Result<()> {
        ensure!(
            self.model.robot.accepts(self.joints),
            "Expected finite values for every driven joint"
        );
        let links = self.model.robot.link_poses(self.joints);
        placements.clear();
        placements.extend(self.model.hulls.iter().map(|hull| {
            let transform = self.base * links[hull.link];
            let pose = parry_pose(&transform);
            HullPlacement {
                bounds: hull.bounds.transform_by(&pose),
                pose,
                centre: transform * hull.centre,
            }
        }));
        Ok(())
    }
}

fn penetrates(
    (first, first_placement): (&Hull, &HullPlacement),
    (second, second_placement): (&Hull, &HullPlacement),
    tolerance: f64,
) -> anyhow::Result<bool> {
    if (first_placement.centre - second_placement.centre).norm_squared()
        > (first.radius + second.radius).powi(2)
        || !first_placement.bounds.intersects(&second_placement.bounds)
    {
        return Ok(false);
    }
    let contact = DefaultQueryDispatcher
        .contact(
            &first_placement.pose.inv_mul(&second_placement.pose),
            &first.shape,
            &second.shape,
            0.0,
        )
        .context("The convex hull query is unsupported")?;
    Ok(contact.is_some_and(|found| found.dist < -tolerance))
}

#[derive(Default)]
pub struct CollisionWorkspace {
    placements: Vec<Vec<HullPlacement>>,
}

impl CollisionWorkspace {
    pub fn scene_collision<'a>(
        &mut self,
        bodies: &[CollisionBody<'a>],
        inter_body_penetration_meters: f64,
    ) -> anyhow::Result<Option<CollisionHit<'a>>> {
        ensure!(
            inter_body_penetration_meters.is_finite() && inter_body_penetration_meters >= 0.0,
            "The inter-body penetration tolerance must be finite and nonnegative"
        );
        if self.placements.len() < bodies.len() {
            self.placements.resize_with(bodies.len(), Vec::new);
        }
        let placements = &mut self.placements[..bodies.len()];
        for (body, hull_placements) in bodies.iter().zip(&mut *placements) {
            body.place_hulls(hull_placements)?;
        }
        let hit =
            |first_body: usize, first: &Hull, second_body: usize, second: &Hull| CollisionHit {
                bodies: [first_body, second_body],
                links: [
                    bodies[first_body].model.robot.link_name(first.link),
                    bodies[second_body].model.robot.link_name(second.link),
                ],
            };
        for (body_index, (body, hull_placements)) in bodies.iter().zip(&*placements).enumerate() {
            let hulls = &body.model.hulls;
            for &(first, second) in &body.model.pairs {
                if penetrates(
                    (&hulls[first], &hull_placements[first]),
                    (&hulls[second], &hull_placements[second]),
                    body.model.penetration_tolerance_meters,
                )? {
                    return Ok(Some(hit(
                        body_index,
                        &hulls[first],
                        body_index,
                        &hulls[second],
                    )));
                }
            }
            for (other_index, (other, other_placements)) in bodies
                .iter()
                .zip(&*placements)
                .enumerate()
                .skip(body_index + 1)
            {
                for first in hulls.iter().zip(hull_placements) {
                    for second in other.model.hulls.iter().zip(other_placements) {
                        if penetrates(first, second, inter_body_penetration_meters)? {
                            return Ok(Some(hit(body_index, first.0, other_index, second.0)));
                        }
                    }
                }
            }
        }
        Ok(None)
    }
}

pub fn scene_collision<'a>(
    bodies: &[CollisionBody<'a>],
    inter_body_penetration_meters: f64,
) -> anyhow::Result<Option<CollisionHit<'a>>> {
    CollisionWorkspace::default().scene_collision(bodies, inter_body_penetration_meters)
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

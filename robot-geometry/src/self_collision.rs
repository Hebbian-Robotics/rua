// CONSTRAINT: Convex hulls fill mesh hollows and permit configured penetration; the guard
// prevents self-driving collisions rather than all physical contact.

use std::sync::Arc;

use anyhow::{Context, bail, ensure};
use nalgebra::{Isometry3, Point3, Vector3};
use parry3d_f64::bounding_volume::{Aabb, BoundingVolume};
use parry3d_f64::math::{Pose, Rot3, Vec3};
use parry3d_f64::query::{DefaultQueryDispatcher, QueryDispatcher};
use parry3d_f64::shape::{ConvexPolyhedron, Shape, SupportMap};

use crate::robot::Robot;

// WHY: Support-point reduction keeps the query cheap but places the hull slightly
// inside the full geometry; collision allowances must account for that approximation.
const HULL_DIRECTIONS: usize = 64;
const SWEEP_DEPTH_LIMIT: u32 = 22;
const SWEEP_INTERVAL_BUDGET: usize = 4096;
const SWEEP_ROUNDING_METERS: f64 = 1e-8;

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
    pairs: Vec<HullPair>,
    penetration_tolerance_meters: f64,
}

struct HullPair {
    first: usize,
    second: usize,
    joint_reach: Vec<f64>,
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
        self.pairs.retain(|pair| {
            let first_link = self.robot.link_name(self.hulls[pair.first].link);
            let second_link = self.robot.link_name(self.hulls[pair.second].link);
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
        let mut joint_reaches = Vec::new();
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
            joint_reaches.push(robot.joint_reach(shape.link, centre.coords.norm() + radius));
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
                    // WHY: Common ancestor joints apply the same rigid motion to
                    // both hulls and cannot change their relative clearance.
                    let joint_reach = joint_reaches[first]
                        .iter()
                        .zip(&joint_reaches[second])
                        .map(|(first, second)| match (first, second) {
                            (Some(reach), None) | (None, Some(reach)) => *reach,
                            _ => 0.0,
                        })
                        .collect();
                    pairs.push(HullPair {
                        first,
                        second,
                        joint_reach,
                    });
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

    // CONSTRAINT: Each supplied speed bounds the corresponding joint derivative
    // over the entire normalized interval, including any reversals.
    pub fn swept_prefix(
        &self,
        joint_speeds: &[f64],
        sample: impl FnMut(f64, &mut [f64]),
    ) -> anyhow::Result<Option<(f64, String)>> {
        ensure!(
            joint_speeds.len() == self.robot.driven_joint_count()
                && joint_speeds
                    .iter()
                    .all(|speed| speed.is_finite() && *speed >= 0.0),
            "Expected finite sweep speed bounds for every driven joint"
        );
        let pair_speeds: Vec<f64> = self
            .pairs
            .iter()
            .map(|pair| {
                pair.joint_reach
                    .iter()
                    .zip(joint_speeds)
                    .map(|(reach, speed)| reach * speed)
                    .sum()
            })
            .collect();
        ensure!(
            pair_speeds.iter().all(|speed| speed.is_finite()),
            "Sweep speed bounds overflowed"
        );
        let mut sweep = Sweep {
            model: self,
            sample,
            joints: vec![0.0; joint_speeds.len()],
            placements: Vec::new(),
            pair_speeds,
            remaining_intervals: SWEEP_INTERVAL_BUDGET,
        };
        sweep.first_unclear_interval(0.0, 1.0, SWEEP_DEPTH_LIMIT)
    }
}

struct Sweep<'a, Sample> {
    model: &'a SelfCollisionModel,
    sample: Sample,
    joints: Vec<f64>,
    placements: Vec<HullPlacement>,
    pair_speeds: Vec<f64>,
    remaining_intervals: usize,
}

impl<Sample: FnMut(f64, &mut [f64])> Sweep<'_, Sample> {
    fn first_unclear_interval(
        &mut self,
        start: f64,
        end: f64,
        depth: u32,
    ) -> anyhow::Result<Option<(f64, String)>> {
        self.remaining_intervals = self.remaining_intervals.saturating_sub(1);
        let middle = f64::midpoint(start, end);
        (self.sample)(middle, &mut self.joints);
        CollisionBody {
            model: self.model,
            joints: &self.joints,
            base: Isometry3::identity(),
        }
        .place_hulls(&mut self.placements)?;
        let mut unclear = None;
        for (pair, speed) in self.model.pairs.iter().zip(&self.pair_speeds) {
            let first = &self.model.hulls[pair.first];
            let second = &self.model.hulls[pair.second];
            let first_placement = &self.placements[pair.first];
            let second_placement = &self.placements[pair.second];
            let travel = speed * (end - start) * 0.5;
            if !sweep_clears_pair(
                (first, first_placement),
                (second, second_placement),
                travel,
                self.model.penetration_tolerance_meters,
            )? {
                unclear = Some(pair);
                break;
            }
        }
        let Some(pair) = unclear else {
            return Ok(None);
        };
        // WHY: A difficult near-contact path must not tie up planning
        // indefinitely. Exhausting subdivision leaves the interval unverified.
        if depth == 0 || self.remaining_intervals == 0 {
            let first = self
                .model
                .robot
                .link_name(self.model.hulls[pair.first].link);
            let second = self
                .model
                .robot
                .link_name(self.model.hulls[pair.second].link);
            return Ok(Some((
                start,
                format!(
                    "Self-collision clearance could not be verified between {first} and {second}"
                ),
            )));
        }
        match self.first_unclear_interval(start, middle, depth - 1)? {
            Some(blocked) => Ok(Some(blocked)),
            None => self.first_unclear_interval(middle, end, depth - 1),
        }
    }
}

fn sweep_clears_pair(
    (first, first_placement): (&Hull, &HullPlacement),
    (second, second_placement): (&Hull, &HullPlacement),
    travel: f64,
    tolerance: f64,
) -> anyhow::Result<bool> {
    if travel == 0.0 {
        return Ok(!penetrates(
            (first, first_placement),
            (second, second_placement),
            tolerance,
        )?);
    }
    let travel = travel + SWEEP_ROUNDING_METERS;
    let centre_distance = (first_placement.centre - second_placement.centre).norm();
    if centre_distance > first.radius + second.radius + travel
        || !first_placement
            .bounds
            .loosened(travel)
            .intersects(&second_placement.bounds)
    {
        return Ok(true);
    }
    let relative = first_placement.pose.inv_mul(&second_placement.pose);
    let contact = DefaultQueryDispatcher
        .contact(
            &relative,
            &first.shape,
            &second.shape,
            travel + first.radius + second.radius,
        )
        .context("The convex hull sweep query is unsupported")?;
    // WHY: Use Parry's normal to bound the full hull projections. Motion
    // can shrink this gap by at most the relative point travel. A gap above
    // minus the allowance bounds the penetration along this separating axis.
    Ok(contact
        .and_then(|contact| contact.normal1.try_normalize())
        .is_some_and(|axis| {
            let first_support = first.shape.local_support_point(axis);
            let second_support = second.shape.support_point(&relative, -axis);
            axis.dot(second_support - first_support) + tolerance >= travel
        }))
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
            for &HullPair { first, second, .. } in &body.model.pairs {
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

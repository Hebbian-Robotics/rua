use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::{Context, anyhow, bail, ensure};
use nalgebra::{Isometry3, Point3, Translation3, Unit, UnitQuaternion, Vector3};
use urdf_rs::{Geometry, JointType, Pose};

use crate::kinematics::{ChainJoint, SerialChain};

fn isometry(pose: &Pose) -> Isometry3<f64> {
    Isometry3::from_parts(
        Translation3::new(pose.xyz[0], pose.xyz[1], pose.xyz[2]),
        UnitQuaternion::from_euler_angles(pose.rpy[0], pose.rpy[1], pose.rpy[2]),
    )
}

struct JointNode {
    parent: usize,
    child: usize,
    origin: Isometry3<f64>,
    axis: Vector3<f64>,
    slot: Option<usize>,
}

pub(crate) struct CollisionShape {
    pub link: usize,
    pub joint: Option<usize>,
    // CONSTRAINT: Cylinder, capsule and sphere box bounds are too coarse for collision admission;
    // only mesh/box vertices are exact here.
    pub exact: bool,
    pub points: Vec<Point3<f64>>,
    centre: Point3<f64>,
    radius: f64,
}

// INVARIANT: Cache keys use exact configurations; tracking tolerances and rounding must not
// reuse geometry for another pose.
struct CachedConfiguration {
    joints: Vec<f64>,
    poses: Arc<[Isometry3<f64>]>,
    lowest_point: Option<f64>,
}

const MAXIMUM_CACHED_CONFIGURATIONS: usize = 8;

pub struct Robot {
    links: Vec<String>,
    nodes: Vec<JointNode>,
    shapes: Vec<CollisionShape>,
    moving_links: Vec<usize>,
    slot_count: usize,
    link_index: HashMap<String, usize>,
    joint_parents: Vec<Option<usize>>,
    configurations: Mutex<VecDeque<CachedConfiguration>>,
}

impl Robot {
    pub fn load(urdf: &Path, driven_joints: &[String]) -> anyhow::Result<Self> {
        let description = urdf_rs::read_file(urdf)
            .with_context(|| format!("Cannot read the URDF {}", urdf.display()))?;
        let links: Vec<String> = description.links.iter().map(|l| l.name.clone()).collect();
        let link_index: HashMap<String, usize> = links
            .iter()
            .enumerate()
            .map(|(index, name)| (name.clone(), index))
            .collect();
        let slot_of: HashMap<&str, usize> = driven_joints
            .iter()
            .enumerate()
            .map(|(slot, name)| (name.as_str(), slot))
            .collect();
        ensure_driven_joints_rotate(&description.joints, driven_joints)?;

        let mut unordered = Vec::new();
        for joint in &description.joints {
            let parent = *link_index
                .get(&joint.parent.link)
                .ok_or_else(|| anyhow!("Unknown parent link {}", joint.parent.link))?;
            let child = *link_index
                .get(&joint.child.link)
                .ok_or_else(|| anyhow!("Unknown child link {}", joint.child.link))?;
            let movable = matches!(
                joint.joint_type,
                JointType::Revolute | JointType::Continuous
            );
            let axis = Vector3::new(joint.axis.xyz[0], joint.axis.xyz[1], joint.axis.xyz[2]);
            unordered.push((
                JointNode {
                    parent,
                    child,
                    origin: isometry(&joint.origin),
                    axis,
                    slot: if movable {
                        slot_of.get(joint.name.as_str()).copied()
                    } else {
                        None
                    },
                },
                joint.name.clone(),
                movable,
            ));
        }

        let mut ordered: Vec<(JointNode, String, bool)> = Vec::new();
        let mut reached = vec![false; links.len()];
        let root = (0..links.len())
            .find(|link| unordered.iter().all(|(node, _, _)| node.child != *link))
            .ok_or_else(|| anyhow!("The URDF has no root link"))?;
        reached[root] = true;
        while !unordered.is_empty() {
            let position = unordered
                .iter()
                .position(|(node, _, _)| reached[node.parent])
                .ok_or_else(|| anyhow!("The URDF's joints do not form a tree"))?;
            let entry = unordered.remove(position);
            reached[entry.0.child] = true;
            ordered.push(entry);
        }

        let mut rides_on: Vec<Option<usize>> = vec![None; links.len()];
        for (index, (node, _, movable)) in ordered.iter().enumerate() {
            rides_on[node.child] = if *movable {
                Some(index)
            } else {
                rides_on[node.parent]
            };
        }
        let moving_links: Vec<usize> = (0..links.len())
            .filter(|link| rides_on[*link].is_some_and(|joint| ordered[joint].0.slot.is_some()))
            .collect();

        let directory = urdf.parent().unwrap_or_else(|| Path::new("."));
        let shapes = collision_shapes(&description.links, &rides_on, directory)?;

        let joint_parents = ordered
            .iter()
            .map(|(node, _, _)| rides_on[node.parent])
            .collect();
        Ok(Self {
            links,
            joint_parents,
            nodes: ordered.into_iter().map(|(node, _, _)| node).collect(),
            shapes,
            moving_links,
            slot_count: driven_joints.len(),
            link_index,
            configurations: Mutex::new(VecDeque::with_capacity(MAXIMUM_CACHED_CONFIGURATIONS)),
        })
    }

    pub(crate) fn link_poses(&self, joints: &[f64]) -> Arc<[Isometry3<f64>]> {
        // INVARIANT: A shared preview or preparation worker must never make control wait.
        if let Ok(configurations) = self.configurations.try_lock()
            && let Some(cached) = configurations.iter().find(|cached| cached.joints == joints)
        {
            return Arc::clone(&cached.poses);
        }
        let mut poses = vec![Isometry3::identity(); self.links.len()];
        for node in &self.nodes {
            let motion = match node.slot {
                Some(slot) => Isometry3::from_parts(
                    Translation3::identity(),
                    UnitQuaternion::from_axis_angle(&Unit::new_normalize(node.axis), joints[slot]),
                ),
                None => Isometry3::identity(),
            };
            poses[node.child] = poses[node.parent] * node.origin * motion;
        }
        let poses: Arc<[Isometry3<f64>]> = poses.into();
        if let Ok(mut configurations) = self.configurations.try_lock()
            && !configurations.iter().any(|cached| cached.joints == joints)
        {
            if configurations.len() == MAXIMUM_CACHED_CONFIGURATIONS {
                configurations.pop_front();
            }
            configurations.push_back(CachedConfiguration {
                joints: joints.to_vec(),
                poses: Arc::clone(&poses),
                lowest_point: None,
            });
        }
        poses
    }

    pub(crate) fn joint_reach(&self, mut link: usize, mut radius: f64) -> Vec<Option<f64>> {
        let mut reach = vec![None; self.slot_count];
        // SAFETY: Downstream translations and the hull radius bound a point's
        // distance from each ancestor joint axis in every configuration.
        for node in self.nodes.iter().rev() {
            if node.child == link {
                if let Some(slot) = node.slot {
                    reach[slot] = Some(radius);
                }
                radius += node.origin.translation.vector.norm();
                link = node.parent;
            }
        }
        reach
    }

    pub fn serial_chain(
        &self,
        tool_link: &str,
        arm_joint_count: usize,
    ) -> anyhow::Result<SerialChain> {
        let mut link = *self
            .link_index
            .get(tool_link)
            .ok_or_else(|| anyhow!("The URDF has no link {tool_link}"))?;
        let mut path = Vec::new();
        while let Some(node) = self.nodes.iter().find(|node| node.child == link) {
            path.push(node);
            link = node.parent;
        }
        path.reverse();
        let mut joints = Vec::new();
        let mut fixed = Isometry3::identity();
        let mut slots = Vec::new();
        for node in path {
            fixed *= node.origin;
            if let Some(slot) = node.slot {
                slots.push(slot);
                joints.push(ChainJoint {
                    before: fixed,
                    axis: Unit::new_normalize(node.axis),
                });
                fixed = Isometry3::identity();
            }
        }
        ensure!(
            slots.iter().copied().eq(0..arm_joint_count),
            "The movable joints on the way to {tool_link} are driven slots {slots:?}, \
             not the arm's joints 0 to {arm_joint_count}"
        );
        Ok(SerialChain::new(joints, fixed))
    }

    #[must_use]
    pub(crate) fn collision_shapes(&self) -> &[CollisionShape] {
        &self.shapes
    }

    #[must_use]
    pub(crate) fn joint_parent(&self, joint: usize) -> Option<usize> {
        self.joint_parents[joint]
    }

    #[must_use]
    pub(crate) fn link_name(&self, link: usize) -> &str {
        &self.links[link]
    }

    #[must_use]
    pub(crate) fn accepts(&self, joints: &[f64]) -> bool {
        joints.len() == self.slot_count && joints.iter().all(|value| value.is_finite())
    }

    #[must_use]
    pub fn driven_joint_count(&self) -> usize {
        self.slot_count
    }

    pub fn link_pose(&self, link: &str, joints: &[f64]) -> anyhow::Result<Isometry3<f64>> {
        if joints.len() != self.slot_count {
            bail!("Wrong number of joint values");
        }
        let index = *self
            .link_index
            .get(link)
            .ok_or_else(|| anyhow!("The URDF has no link {link}"))?;
        Ok(self.link_poses(joints)[index])
    }

    pub fn lowest_point(&self, joints: &[f64]) -> anyhow::Result<f64> {
        if joints.len() != self.slot_count || joints.iter().any(|value| !value.is_finite()) {
            bail!("Expected finite values for every driven joint");
        }
        if let Ok(configurations) = self.configurations.try_lock()
            && let Some(lowest) = configurations
                .iter()
                .find(|cached| cached.joints == joints)
                .and_then(|cached| cached.lowest_point)
        {
            return Ok(lowest);
        }
        let poses = self.link_poses(joints);
        let mut lowest = self
            .moving_links
            .iter()
            .map(|link| poses[*link].translation.vector.z)
            .fold(f64::INFINITY, f64::min);
        let moving_shapes = || {
            self.shapes
                .iter()
                .filter(|shape| self.moving_links.contains(&shape.link))
        };
        // INVARIANT: A shape's lowest corner is no higher than its centre and no lower
        // than its sphere's bottom, so a shape whose sphere clears the lowest height
        // found so far cannot lower it and its corners are skipped.
        for shape in moving_shapes() {
            lowest = lowest.min((poses[shape.link] * shape.centre).z);
        }
        for shape in moving_shapes() {
            let pose = &poses[shape.link];
            if (pose * shape.centre).z - shape.radius >= lowest {
                continue;
            }
            let row = pose
                .rotation
                .to_rotation_matrix()
                .into_inner()
                .row(2)
                .into_owned();
            let offset = pose.translation.vector.z;
            for point in &shape.points {
                lowest =
                    lowest.min(row[0] * point.x + row[1] * point.y + row[2] * point.z + offset);
            }
        }
        if let Ok(mut configurations) = self.configurations.try_lock()
            && let Some(cached) = configurations
                .iter_mut()
                .find(|cached| cached.joints == joints)
        {
            cached.lowest_point = Some(lowest);
        }
        Ok(lowest)
    }
}

// SAFETY: A driven prismatic or fixed joint would be posed as stationary and make the
// table guard check the wrong geometry.
fn ensure_driven_joints_rotate(
    joints: &[urdf_rs::Joint],
    driven_joints: &[String],
) -> anyhow::Result<()> {
    for name in driven_joints {
        let joint = joints
            .iter()
            .find(|joint| &joint.name == name)
            .ok_or_else(|| anyhow!("The URDF has no joint {name}"))?;
        ensure!(
            matches!(
                joint.joint_type,
                JointType::Revolute | JointType::Continuous
            ),
            "Driven joint {name} is {:?}; only revolute and continuous joints can be driven",
            joint.joint_type
        );
    }
    Ok(())
}

fn collision_shapes(
    links: &[urdf_rs::Link],
    rides_on: &[Option<usize>],
    directory: &Path,
) -> anyhow::Result<Vec<CollisionShape>> {
    let mut shapes = Vec::new();
    for (link, description_link) in links.iter().enumerate() {
        for collision in &description_link.collision {
            let placement = isometry(&collision.origin);
            let points = shape_points(&collision.geometry, directory)?;
            let points = hull_vertices(points.iter().map(|point| placement * point).collect());
            let centre = Point3::from(
                points
                    .iter()
                    .fold(Vector3::zeros(), |sum, point| sum + point.coords)
                    / crate::numeric::count(points.len().max(1)),
            );
            let radius = points
                .iter()
                .map(|point| (point - centre).norm())
                .fold(0.0, f64::max);
            shapes.push(CollisionShape {
                link,
                joint: rides_on[link],
                exact: matches!(
                    collision.geometry,
                    Geometry::Mesh { .. } | Geometry::Box { .. }
                ),
                points,
                centre,
                radius,
            });
        }
    }
    Ok(shapes)
}

// WHY: Convex-hull vertices preserve the lowest point and collision hull with fewer
// points; flat meshes without a hull retain their original points.
fn hull_vertices(points: Vec<Point3<f64>>) -> Vec<Point3<f64>> {
    let as_parry: Vec<parry3d_f64::math::Vec3> = points
        .iter()
        .map(|point| parry3d_f64::math::Vec3::new(point.x, point.y, point.z))
        .collect();
    match parry3d_f64::shape::ConvexPolyhedron::from_convex_hull(&as_parry) {
        Some(hull) => hull
            .points()
            .iter()
            .map(|point| Point3::new(point.x, point.y, point.z))
            .collect(),
        None => points,
    }
}

fn shape_points(geometry: &Geometry, directory: &Path) -> anyhow::Result<Vec<Point3<f64>>> {
    let corners = |half: [f64; 3]| -> Vec<Point3<f64>> {
        let mut points = Vec::new();
        for x in [-half[0], half[0]] {
            for y in [-half[1], half[1]] {
                for z in [-half[2], half[2]] {
                    points.push(Point3::new(x, y, z));
                }
            }
        }
        points
    };
    match geometry {
        Geometry::Mesh { filename, scale } => {
            let path = directory.join(filename.trim_start_matches("package://"));
            let mut file = std::fs::File::open(&path)
                .with_context(|| format!("Cannot open mesh {}", path.display()))?;
            let mesh = stl_io::read_stl(&mut file)
                .with_context(|| format!("Cannot read mesh {}", path.display()))?;
            let scale = scale.as_ref().map_or([1.0; 3], |scale| scale.0);
            Ok(mesh
                .vertices
                .iter()
                .map(|vertex| {
                    Point3::new(
                        f64::from(vertex[0]) * scale[0],
                        f64::from(vertex[1]) * scale[1],
                        f64::from(vertex[2]) * scale[2],
                    )
                })
                .collect())
        }
        Geometry::Box { size } => Ok(corners([size[0] / 2.0, size[1] / 2.0, size[2] / 2.0])),
        Geometry::Cylinder { radius, length } => Ok(corners([*radius, *radius, length / 2.0])),
        Geometry::Capsule { radius, length } => {
            Ok(corners([*radius, *radius, length / 2.0 + radius]))
        }
        Geometry::Sphere { radius } => Ok(corners([*radius; 3])),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_serial_chain_puts_the_tool_where_the_robot_does() {
        let urdf = r#"<robot name="arm">
          <link name="base"/><link name="upper"/><link name="mount"/><link name="forearm"/><link name="tool"/>
          <joint name="shoulder" type="revolute"><parent link="base"/><child link="upper"/>
            <origin xyz="0.1 0.0 0.2" rpy="0.0 0.0 0.3"/><axis xyz="0 0 1"/>
            <limit lower="-2" upper="2" effort="1" velocity="1"/></joint>
          <joint name="fixed_mount" type="fixed"><parent link="upper"/><child link="mount"/>
            <origin xyz="0.0 0.05 0.3" rpy="0.4 0.0 0.0"/></joint>
          <joint name="elbow" type="revolute"><parent link="mount"/><child link="forearm"/>
            <origin xyz="0.02 0.0 0.1" rpy="0.0 0.2 0.0"/><axis xyz="0 1 0"/>
            <limit lower="-2" upper="2" effort="1" velocity="1"/></joint>
          <joint name="tip" type="fixed"><parent link="forearm"/><child link="tool"/>
            <origin xyz="0.0 0.0 0.25" rpy="0.0 0.0 0.1"/></joint>
        </robot>"#;
        let path = std::env::temp_dir().join(format!("chain-{}.urdf", std::process::id()));
        std::fs::write(&path, urdf).unwrap();
        let robot = Robot::load(&path, &["shoulder".to_owned(), "elbow".to_owned()]);
        let _ = std::fs::remove_file(path);
        let robot = robot.unwrap();
        let chain = robot.serial_chain("tool", 2).unwrap();
        for joints in [
            [0.0, 0.0],
            [0.7, -0.4],
            [-1.3, 1.1],
            [0.0, 0.0],
            [0.0, 1e-8],
            [1e-8, 0.0],
            [0.0, 0.0],
        ] {
            let expected = robot.link_pose("tool", &joints).unwrap();
            let found = chain.tool_pose(&joints);
            assert!((expected.translation.vector - found.translation.vector).norm() < 1e-12);
            assert!(expected.rotation.angle_to(&found.rotation) < 1e-12);
        }
        assert!(robot.serial_chain("tool", 3).is_err());
    }

    #[test]
    fn a_driven_joint_that_slides_is_refused() {
        let urdf = r#"<robot name="slider">
          <link name="base"/><link name="carriage"/>
          <joint name="slide" type="prismatic"><parent link="base"/><child link="carriage"/>
            <axis xyz="1 0 0"/><limit lower="0" upper="1" effort="1" velocity="1"/></joint>
        </robot>"#;
        let path = std::env::temp_dir().join(format!("slider-{}.urdf", std::process::id()));
        std::fs::write(&path, urdf).unwrap();
        let refused = Robot::load(&path, &["slide".to_owned()]).err();
        let _ = std::fs::remove_file(path);
        let message = refused
            .expect("a prismatic joint cannot be driven")
            .to_string();
        assert!(message.contains("only revolute"), "{message}");
    }
}

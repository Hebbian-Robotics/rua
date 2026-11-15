//! The arm's kinematic tree and collision geometry, read from the robot's URDF.
//!
//! Shared forward kinematics for tool poses, lowest-point queries and collision
//! geometry. The executor applies its own table and path admission policy using
//! these queries; the model has no station configuration or hardware I/O.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, anyhow, bail, ensure};
use nalgebra::{Isometry3, Point3, Translation3, Unit, UnitQuaternion, Vector3};
use urdf_rs::{Geometry, JointType, Pose};

use crate::kinematics::{ChainJoint, SerialChain};

/// A URDF pose: translation, then rotation by roll, pitch, yaw about fixed axes.
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
    /// Which entry of the joint-value vector drives it; `None` when fixed or unlisted.
    slot: Option<usize>,
}

/// Points that bound a collision shape, in the link frame.
pub(crate) struct CollisionShape {
    pub link: usize,
    /// The movable joint the shape rides on; `None` for the fixed base.
    pub joint: Option<usize>,
    /// The points are the shape's own vertices; cylinders, capsules and spheres
    /// are only bounded by a box, which is too coarse for collision checks.
    pub exact: bool,
    pub points: Vec<Point3<f64>>,
}

pub struct Robot {
    links: Vec<String>,
    nodes: Vec<JointNode>,
    shapes: Vec<CollisionShape>,
    /// Links that move with one of the driven joints, whose geometry and frame
    /// origin count toward the arm's lowest point.
    moving_links: Vec<usize>,
    slot_count: usize,
    link_index: HashMap<String, usize>,
    /// Each movable joint's name, and the movable joint it hangs from.
    joint_parents: Vec<Option<usize>>,
}

impl Robot {
    /// Load the URDF, driving `driven_joints` in order (the arm's joints, then
    /// the gripper when it has one). Any other movable joint stays at zero.
    ///
    /// # Errors
    /// The URDF is unreadable, lacks a driven joint, drives a joint that is not
    /// revolute, or references a missing mesh.
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

        // Parents before children, from the one link nothing is a child of.
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

        // The movable joint each link rides on, walking up through fixed joints.
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
        })
    }

    /// Each link's pose in the URDF's root frame for these joint values.
    pub(crate) fn link_poses(&self, joints: &[f64]) -> Vec<Isometry3<f64>> {
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
        poses
    }

    /// The serial chain from the URDF root to `tool_link` through the arm's
    /// first `arm_joint_count` driven joints, which must be the chain's movable
    /// joints in order.
    ///
    /// # Errors
    /// The link is unknown, or the movable joints on the way to it are not the
    /// arm's driven joints in order.
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

    /// The movable joint that `joint` hangs from, `None` for the base.
    #[must_use]
    pub(crate) fn joint_parent(&self, joint: usize) -> Option<usize> {
        self.joint_parents[joint]
    }

    #[must_use]
    pub(crate) fn link_name(&self, link: usize) -> &str {
        &self.links[link]
    }

    /// Whether the values are one finite number per driven joint.
    #[must_use]
    pub(crate) fn accepts(&self, joints: &[f64]) -> bool {
        joints.len() == self.slot_count && joints.iter().all(|value| value.is_finite())
    }

    /// Number of driven joints expected by pose and collision queries.
    #[must_use]
    pub fn driven_joint_count(&self) -> usize {
        self.slot_count
    }

    /// The pose of a link in the root frame.
    ///
    /// # Errors
    /// The joint values do not match the driven joints, or the link is unknown.
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

    /// Height of the lowest moving part: collision-geometry points and the origins
    /// of the moving links, in the root frame.
    ///
    /// # Errors
    /// The joint values do not match the driven joints.
    pub fn lowest_point(&self, joints: &[f64]) -> anyhow::Result<f64> {
        if joints.len() != self.slot_count || joints.iter().any(|value| !value.is_finite()) {
            bail!("Expected finite values for every driven joint");
        }
        let poses = self.link_poses(joints);
        let mut lowest = self
            .moving_links
            .iter()
            .map(|link| poses[*link].translation.vector.z)
            .fold(f64::INFINITY, f64::min);
        for shape in &self.shapes {
            if !self.moving_links.contains(&shape.link) {
                continue;
            }
            let pose = &poses[shape.link];
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
        Ok(lowest)
    }
}

/// Every driven joint exists and rotates. Only rotation is modelled: a driven
/// prismatic or fixed joint would otherwise be posed as fixed, and the table
/// guard would check the wrong pose.
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

/// Every collision shape of every link, with the movable joint it rides on.
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
            shapes.push(CollisionShape {
                link,
                joint: rides_on[link],
                exact: matches!(
                    collision.geometry,
                    Geometry::Mesh { .. } | Geometry::Box { .. }
                ),
                points: hull_vertices(points.iter().map(|point| placement * point).collect()),
            });
        }
    }
    Ok(shapes)
}

/// The corners of the points' convex hull, which bound them exactly as all of
/// them do: the lowest point and the self-collision hull are the same, from far
/// fewer points. Points with no hull of their own (a flat mesh) stay as they are.
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

/// Points that bound a collision shape: a mesh's vertices, else its box corners.
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
        for joints in [[0.0, 0.0], [0.7, -0.4], [-1.3, 1.1]] {
            let expected = robot.link_pose("tool", &joints).unwrap();
            let found = chain.tool_pose(&joints);
            assert!((expected.translation.vector - found.translation.vector).norm() < 1e-12);
            assert!(expected.rotation.angle_to(&found.rotation) < 1e-12);
        }
        // Asking for more arm joints than lie on the way to the tool is refused.
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

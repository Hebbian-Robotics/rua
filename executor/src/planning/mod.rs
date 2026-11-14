//! Planning for the arm's clients: a target turned into the plan the arm runs,
//! and what the arm could reach toward one, from the arm's own model.
//!
//! Every check runs before the arm moves. Joint targets are clamped into the
//! limits, a pose is solved by inverse kinematics from the previous corner, and
//! the straight joint line to each corner is cut short where the self-collision
//! or table check first stops it. A pose no joints reach exactly runs to the
//! closest pose the solver found, and the plan says how far short that is. Only
//! malformed targets, a pose the arm cannot move toward at all, and a start the
//! arm cannot leave are typed refusals.

mod advice;
pub mod orientation;
mod projection;
#[cfg(test)]
mod tests;

use anyhow::Context;
use nalgebra::{UnitQuaternion, Vector3};

use crate::config::ExecutorConfig;
use crate::control_loop::{build_guard, build_self_collision, load_shared_robot};
use crate::ik::IkSolver;
use crate::joint_limits::motion_speed;
use crate::robot::PathCheck;
use crate::self_collision::SelfCollisionGuard;
use crate::surface_guard::SurfaceGuard;
use hebbian_contracts::{
    CornerPlan, IkFailure, IkPose, IkResult, JawClosingAxis, OrientationGoal, PathLimited,
    PlannedMotion, PoseDescription, PoseLimited, ProjectedPose, ProjectionOutcome, RejectionReason,
    Segment, Waypoint,
};
use orientation::{orientation_summary, pose_orientation_quaternion, xyzw};
/// The joint change below which a solve's closest pose counts as where the arm
/// already is: it cannot move toward the target at all.
const NO_MOVEMENT_JOINT_RADIANS: f64 = 1e-6;

/// Why a target was not planned.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanRefusal {
    pub reason: RejectionReason,
    pub detail: String,
}

fn refusal(reason: RejectionReason, detail: impl Into<String>) -> PlanRefusal {
    PlanRefusal {
        reason,
        detail: detail.into(),
    }
}

/// The distal rotation about the tool axis, which alone sets the jaw yaw.
const WRIST_ROLL_JOINT_NAME: &str = "wrist_roll";

/// The wrist roll among the arm's joints; the URDF may prefix it with the arm's
/// name, as `left_wrist_roll`.
fn wrist_roll_index(joint_names: &[String]) -> Option<usize> {
    joint_names.iter().position(|name| {
        name == WRIST_ROLL_JOINT_NAME
            || name
                .strip_suffix(WRIST_ROLL_JOINT_NAME)
                .is_some_and(|prefix| prefix.ends_with('_'))
    })
}

/// An arm's planner: its model, limits and checks.
pub struct Planner {
    solver: IkSolver,
    joint_names: Vec<String>,
    lower: Vec<f64>,
    upper: Vec<f64>,
    default_joint_speed: f64,
    max_joint_speed: f64,
    self_collision: Option<SelfCollisionGuard>,
    surface: Option<SurfaceGuard>,
    jaw_axis: JawClosingAxis,
    wrist_roll: Option<usize>,
}

impl Planner {
    /// The planner for the arm a config describes: `None` when it names no tool
    /// link to plan poses for.
    ///
    /// # Errors
    /// The URDF cannot be read, or the tool is not at the end of the arm's joints.
    pub fn for_arm(config: &ExecutorConfig) -> anyhow::Result<Option<Self>> {
        if config.tool_link.is_none() {
            return Ok(None);
        }
        let robot = load_shared_robot(config)?.context("A tool link loads the robot model")?;
        let solver =
            IkSolver::for_arm(config, &robot)?.context("A tool link gives a pose solver")?;
        let joint_names = config
            .driven_joints
            .get(..config.joint_count)
            .context("The driven joints start with every arm joint")?
            .to_vec();
        Ok(Some(Self {
            solver,
            wrist_roll: wrist_roll_index(&joint_names),
            joint_names,
            lower: config.lower_limits.clone(),
            upper: config.upper_limits.clone(),
            default_joint_speed: config.default_joint_speed,
            max_joint_speed: config.max_joint_speed,
            self_collision: build_self_collision(config, Some(&robot))?,
            surface: build_guard(config, Some(&robot))?,
            jaw_axis: config.jaw_closing_axis,
        }))
    }

    #[must_use]
    pub fn solver(&self) -> &IkSolver {
        &self.solver
    }

    /// One segment per waypoint, each ending at rest at its corner, from `start`.
    ///
    /// A segment whose straight joint line would collide or drive into the
    /// table is cut short at its last clear sample, the waypoints after it are
    /// left out, and the plan says why.
    ///
    /// # Errors
    /// A waypoint is malformed or out of reach, or the arm cannot leave a corner
    /// without colliding or driving into the table.
    pub fn plan(
        &self,
        start: &[f64],
        waypoints: &[Waypoint],
        requested_speed: Option<f64>,
    ) -> Result<PlannedMotion, PlanRefusal> {
        if waypoints.is_empty() {
            return Err(refusal(
                RejectionReason::InvalidTarget,
                "A move needs at least one waypoint",
            ));
        }
        self.require_joint_shape(start)?;
        let joint_speed = motion_speed(
            requested_speed,
            self.default_joint_speed,
            self.max_joint_speed,
        )
        .map_err(|invalid| refusal(RejectionReason::InvalidTarget, invalid.to_string()))?;
        let mut segments = Vec::new();
        let mut previous = start.to_vec();
        for (index, waypoint) in waypoints.iter().enumerate() {
            let label = format!("waypoint_{index}");
            let (corner, target_position, target_quaternion, pose_limited) = match waypoint {
                Waypoint::Joints { joints } => (self.joint_target(joints)?, None, None, None),
                Waypoint::Pose {
                    position,
                    orientation,
                } => {
                    let quaternion = pose_orientation_quaternion(*orientation, self.jaw_axis)
                        .map_err(|why| refusal(RejectionReason::InvalidTarget, why.to_string()))?;
                    let (corner, pose_limited) =
                        self.solve_waypoint(index, &previous, *position, quaternion)?;
                    (corner, Some(*position), Some(quaternion), pose_limited)
                }
            };
            let (stop, path_limited) = self.clear_prefix(&previous, &corner)?;
            if path_limited.is_none() && pose_limited.is_none() {
                segments.push(segment(
                    label,
                    previous,
                    corner.clone(),
                    target_position,
                    target_quaternion,
                ));
                previous = corner;
                continue;
            }
            // The stop is not the requested pose, so the segment does not claim
            // it, and later waypoints, meant to follow that pose, are left out.
            segments.push(segment(label, previous, stop, None, None));
            return Ok(PlannedMotion {
                plan: CornerPlan {
                    segments,
                    peak_speed: joint_speed,
                },
                path_limited,
                pose_limited,
            });
        }
        Ok(PlannedMotion {
            plan: CornerPlan {
                segments,
                peak_speed: joint_speed,
            },
            path_limited: None,
            pose_limited: None,
        })
    }

    /// The tool's pose at these joints, which may be outside the limits, in the
    /// world frame.
    ///
    /// # Errors
    /// The joints are not one finite value per arm joint.
    pub fn describe(&self, joints: &[f64]) -> Result<PoseDescription, String> {
        self.require_joint_shape(joints)
            .map_err(|refused| refused.detail)?;
        let chain = self.solver.chain();
        let tool = chain.tool_pose(joints);
        Ok(PoseDescription {
            position: tool.translation.vector.into(),
            quaternion: xyzw(&tool.rotation),
            orientation: orientation_summary(&tool.rotation, self.jaw_axis, None),
            model_points: chain.model_points(joints),
        })
    }

    fn require_joint_shape(&self, joints: &[f64]) -> Result<(), PlanRefusal> {
        let expected = self.joint_names.len();
        if joints.len() != expected {
            return Err(refusal(
                RejectionReason::InvalidTarget,
                format!("Expected {expected} joints, got {}", joints.len()),
            ));
        }
        if !joints.iter().all(|value| value.is_finite()) {
            return Err(refusal(
                RejectionReason::InvalidTarget,
                "Joint positions must be finite",
            ));
        }
        Ok(())
    }

    /// The joints clamped into the limits: the one joint-limit check of a move,
    /// against the limits the solver keeps every joint inside, so its solutions
    /// and a target exactly on a bound pass unchanged.
    fn joint_target(&self, joints: &[f64]) -> Result<Vec<f64>, PlanRefusal> {
        self.require_joint_shape(joints)?;
        Ok(joints
            .iter()
            .zip(self.lower.iter().zip(&self.upper))
            .map(|(value, (lower, upper))| value.clamp(*lower, *upper))
            .collect())
    }

    /// Joints that put the tool at the pose, solved from the previous corner, or
    /// the closest pose the solver found and how far short of the pose it is.
    ///
    /// # Errors
    /// The solver refuses the request, or its closest pose is where the arm
    /// already is, so the arm cannot move toward the target at all.
    fn solve_waypoint(
        &self,
        index: usize,
        previous: &[f64],
        position: [f64; 3],
        quaternion: [f64; 4],
    ) -> Result<(Vec<f64>, Option<PoseLimited>), PlanRefusal> {
        let solved = self
            .solver
            .solve(position, OrientationGoal::Required { quaternion }, previous)
            .map_err(|why| refusal(RejectionReason::InvalidTarget, why.to_string()))?;
        match solved {
            IkResult::Solved(pose) => Ok((self.joint_target(&pose.joints)?, None)),
            IkResult::Failed { reason, best } => {
                let advice = self.unreachable_advice(previous, position, quaternion);
                let moves = best
                    .joints
                    .iter()
                    .zip(previous)
                    .any(|(goal, was)| (goal - was).abs() > NO_MOVEMENT_JOINT_RADIANS);
                if !moves {
                    return Err(refusal(
                        RejectionReason::Unreachable,
                        format!("{}; {advice}", not_reached(reason, &best)),
                    ));
                }
                Ok((
                    self.joint_target(&best.joints)?,
                    Some(PoseLimited {
                        waypoint: index,
                        reason,
                        position_shortfall_meters: best.position_error_meters,
                        orientation_shortfall_radians: best.orientation_error_radians,
                        advice,
                    }),
                ))
            }
        }
    }

    /// How far along the straight joint line from `start` to `goal` the arm may
    /// go before it would fold into itself or drive down into the table.
    fn path_check(
        &self,
        start: &[f64],
        goal: &[f64],
        gripper_angle: Option<f64>,
    ) -> anyhow::Result<PathCheck> {
        let collision = match &self.self_collision {
            Some(guard) => guard.safe_prefix(start, goal, gripper_angle)?,
            None => PathCheck::Clear,
        };
        let table = match (&self.surface, &collision) {
            (None, _) | (_, PathCheck::StartRefused { .. }) => PathCheck::Clear,
            (Some(surface), PathCheck::StopsAt { last_safe, .. }) => {
                surface.safe_prefix(start, last_safe, gripper_angle)?
            }
            (Some(surface), PathCheck::Clear) => surface.safe_prefix(start, goal, gripper_angle)?,
        };
        Ok(if table == PathCheck::Clear {
            collision
        } else {
            table
        })
    }

    /// How far along the straight joint line from `start` to `goal` a move may
    /// run: all of it, or up to the last clear sample and why it stops there, as
    /// the preview cuts its ghost. Refused only when the arm cannot leave `start`.
    fn clear_prefix(
        &self,
        start: &[f64],
        goal: &[f64],
    ) -> Result<(Vec<f64>, Option<PathLimited>), PlanRefusal> {
        match self.path_check(start, goal, None) {
            Ok(PathCheck::Clear) => Ok((goal.to_vec(), None)),
            Ok(PathCheck::StopsAt { last_safe, reason }) if last_safe.as_slice() != start => {
                Ok((last_safe, Some(PathLimited { reason })))
            }
            Ok(PathCheck::StartRefused { reason } | PathCheck::StopsAt { reason, .. }) => {
                Err(refusal(RejectionReason::InvalidTarget, reason))
            }
            Err(error) => Err(refusal(
                RejectionReason::InvalidTarget,
                format!("{error:#}"),
            )),
        }
    }

    fn tool_rotation(&self, joints: &[f64]) -> UnitQuaternion<f64> {
        self.solver.chain().tool_pose(joints).rotation
    }

    /// The pose at `joints` as a projection reports it, measured against the
    /// target's position and its orientation when one was asked for.
    fn projected(
        &self,
        joints: Vec<f64>,
        target_position: [f64; 3],
        requested: Option<&UnitQuaternion<f64>>,
    ) -> ProjectedPose {
        let tool = self.solver.chain().tool_pose(&joints);
        ProjectedPose {
            position: tool.translation.vector.into(),
            quaternion: xyzw(&tool.rotation),
            orientation: orientation::tool_orientation(&tool.rotation, self.jaw_axis),
            position_error_meters: (tool.translation.vector - Vector3::from(target_position))
                .norm(),
            orientation_error_radians: requested.map(|wanted| tool.rotation.angle_to(wanted)),
            joints,
        }
    }

    /// Solve toward a position, with the orientation asked for, or with the
    /// solver's choice near the one held at `seed` when none is.
    fn solve_toward(
        &self,
        seed: &[f64],
        position: [f64; 3],
        requested: Option<&UnitQuaternion<f64>>,
    ) -> ProjectionOutcome {
        let goal = match requested {
            Some(rotation) => OrientationGoal::Required {
                quaternion: xyzw(rotation),
            },
            None => OrientationGoal::Free,
        };
        match self.solver.solve(position, goal, seed) {
            Err(why) => ProjectionOutcome::Unavailable {
                reason: why.to_string(),
            },
            Ok(IkResult::Solved(pose)) => ProjectionOutcome::Projected {
                pose: self.projected(pose.joints, position, requested),
                within_tolerance: true,
                reason: None,
            },
            Ok(IkResult::Failed { reason, best }) => ProjectionOutcome::Projected {
                pose: self.projected(best.joints, position, requested),
                within_tolerance: false,
                reason: Some(format!(
                    "Not reachable exactly: {}; a move runs to this closest pose the solver \
                     found, {:.1} mm{} from the target",
                    shortfall_cause(reason),
                    best.position_error_meters * 1000.0,
                    if requested.is_some() {
                        format!(
                            " and {:.1} degrees",
                            best.orientation_error_radians.to_degrees()
                        )
                    } else {
                        String::new()
                    }
                )),
            },
        }
    }
}

/// What a corner plan's segment from `from` to `to` looks like.
fn segment(
    label: String,
    from: Vec<f64>,
    to: Vec<f64>,
    target_position: Option<[f64; 3]>,
    target_quaternion: Option<[f64; 4]>,
) -> Segment {
    Segment {
        label,
        corners: vec![from, to],
        minimum_duration_seconds: None,
        target_position,
        target_quaternion,
    }
}

/// Why a solve fell short, in words.
fn shortfall_cause(reason: IkFailure) -> &'static str {
    match reason {
        IkFailure::AtJointLimit => "a joint stops at its limit short of the pose",
        IkFailure::Stalled => "the search stalled short of the pose",
        IkFailure::IterationLimit => "the search ran out of iterations",
        IkFailure::OutOfReach => "the pose is beyond the arm's reach",
    }
}

/// Why a solve that leaves the arm where it is fell short, for a refusal.
fn not_reached(reason: IkFailure, best: &IkPose) -> String {
    format!(
        "No joints move the tool toward the pose: {}; the closest is {:.1} mm and {:.3} rad \
         away; arm has not moved",
        shortfall_cause(reason),
        best.position_error_meters * 1000.0,
        best.orientation_error_radians
    )
}

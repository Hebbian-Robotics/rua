mod advice;
pub use crate::orientation;
mod projection;
#[cfg(test)]
mod tests;

use anyhow::Context;
use nalgebra::{UnitQuaternion, Vector3};

use crate::config::ControllerConfig;
use crate::ik::IkSolver;
use crate::motion_guards::{build_guard, build_self_collision, load_shared_robot};
use crate::motion_values::JointSpeed;
use crate::robot::PathCheck;
use crate::self_collision::SelfCollisionGuard;
use crate::surface_guard::SurfaceGuard;
use hebbian_contracts::{
    CornerPlan, IkFailure, IkPose, IkResult, JawClosingAxis, OrientationGoal, PathLimited,
    PlannedMotion, PoseDescription, PoseLimited, ProjectedPose, ProjectionOutcome, RejectionReason,
    Segment, Waypoint,
};
use orientation::{orientation_summary, pose_orientation_quaternion, xyzw};
const NO_MOVEMENT_JOINT_RADIANS: f64 = 1e-6;

use crate::arm::{Rejection, rejection};

const WRIST_ROLL_JOINT_NAME: &str = "wrist_roll";

fn wrist_roll_index(joint_names: &[String]) -> Option<usize> {
    joint_names.iter().position(|name| {
        name == WRIST_ROLL_JOINT_NAME
            || name
                .strip_suffix(WRIST_ROLL_JOINT_NAME)
                .is_some_and(|prefix| prefix.ends_with('_'))
    })
}

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
    pub fn for_arm(config: &ControllerConfig) -> anyhow::Result<Option<Self>> {
        if config.tool_link.is_none() {
            return Ok(None);
        }
        let robot = load_shared_robot(config)?.context("A tool link loads the robot model")?;
        let solver = crate::ik::solver_for_arm(config, &robot)?
            .context("A tool link gives a pose solver")?;
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

    // SAFETY: Cut a colliding segment short at its last clear sample and discard subsequent waypoints. Refuse a start the arm cannot safely leave.
    pub fn plan(
        &self,
        start: &[f64],
        waypoints: &[Waypoint],
        requested_speed: Option<JointSpeed>,
        gripper_angle: Option<f64>,
    ) -> Result<PlannedMotion, Rejection> {
        if waypoints.is_empty() {
            return Err(rejection(
                RejectionReason::InvalidTarget,
                "A move needs at least one waypoint",
            ));
        }
        self.require_joint_shape(start)?;
        let joint_speed = JointSpeed::for_request(
            requested_speed,
            self.default_joint_speed,
            self.max_joint_speed,
        )
        .map_err(|invalid| rejection(RejectionReason::InvalidTarget, invalid.to_string()))?;
        let mut segments = Vec::new();
        let mut previous = start.to_vec();
        for (index, waypoint) in waypoints.iter().enumerate() {
            let label = format!("waypoint_{index}");
            let (mut corner, destination_position, destination_quaternion, pose_limited) =
                match waypoint {
                    Waypoint::Joints { joints } => {
                        let target = crate::joint_limits::JointTarget::parse(
                            joints.as_slice(),
                            self.joint_names.len(),
                            &self.lower,
                            &self.upper,
                        )
                        .map_err(|error| {
                            rejection(RejectionReason::InvalidTarget, error.to_string())
                        })?;
                        (target.as_slice().to_vec(), None, None, None)
                    }
                    Waypoint::Pose {
                        position,
                        orientation,
                    } => {
                        let quaternion = pose_orientation_quaternion(*orientation, self.jaw_axis)
                            .map_err(|why| {
                            rejection(RejectionReason::InvalidTarget, why.to_string())
                        })?;
                        let (corner, pose_limited) =
                            self.solve_waypoint(index, &previous, *position, quaternion)?;
                        (corner, Some(*position), Some(quaternion), pose_limited)
                    }
                };
            let mut path = self.path_check(&previous, &corner, gripper_angle);
            if matches!(path, Ok(PathCheck::StopsAt { .. }))
                && pose_limited.is_none()
                && let (Some(position), Some(quaternion)) =
                    (destination_position, destination_quaternion)
                && let Some(refined) =
                    self.refine_clear_pose(&previous, &corner, position, quaternion, gripper_angle)
            {
                corner = refined;
                path = Ok(PathCheck::Clear);
            }
            let (stop, path_limited) = Self::clear_prefix(&previous, corner, path)?;
            if path_limited.is_none() && pose_limited.is_none() {
                segments.push(segment(
                    label,
                    previous,
                    stop.clone(),
                    destination_position,
                    destination_quaternion,
                ));
                previous = stop;
                continue;
            }
            // INVARIANT: The stop is not the requested pose, so the segment does not claim
            // it, and later waypoints, meant to follow that pose, are left out.
            segments.push(segment(label, previous, stop, None, None));
            return Ok(PlannedMotion {
                plan: CornerPlan {
                    segments,
                    peak_speed: joint_speed.radians_per_second(),
                    trajectory_limits: None,
                },
                path_limited,
                pose_limited,
            });
        }
        Ok(PlannedMotion {
            plan: CornerPlan {
                segments,
                peak_speed: joint_speed.radians_per_second(),
                trajectory_limits: None,
            },
            path_limited: None,
            pose_limited: None,
        })
    }

    // SAFETY: Check the curve the controller will sample, including the possible jaw
    // positions while an opening command is still in flight.
    pub(crate) fn check_continuous_path(
        &self,
        plan: &CornerPlan,
        config: &ControllerConfig,
        measured_jaw: Option<f64>,
        initial_jaw_goal: Option<f64>,
    ) -> anyhow::Result<()> {
        let mut limits = config.motion.clone();
        if let Some(trajectory_limits) = plan.trajectory_limits {
            limits.acceleration_limit = trajectory_limits.acceleration_limit;
            limits.jerk_limit = trajectory_limits.jerk_limit;
        }
        let mut path = crate::corner_path::ContinuousPath::new(
            plan,
            &limits,
            config.control_timing.maximum_step_seconds,
        )?;
        path.check_joint_bounds(&plan.segments[0].corners[0], &self.lower, &self.upper)?;
        let mut reference =
            crate::reference_filter::Reference::at_rest(&plan.segments[0].corners[0], None);
        let mut jaw_range = measured_jaw.map(|angle| (angle, angle));
        for index in 0..plan.segments.len() {
            let jaw_command = if index == 0 {
                initial_jaw_goal
            } else {
                plan.segments[index - 1]
                    .pass_through
                    .and_then(|point| point.gripper_angle)
                    .map(hebbian_contracts::JawAngle::radians)
            };
            if let Some(angle) = jaw_command {
                let (minimum, maximum) = jaw_range.as_mut().ok_or_else(|| {
                    anyhow::anyhow!("Missing jaw measurement for a continuous move")
                })?;
                *minimum = minimum.min(angle);
                *maximum = maximum.max(angle);
            }
            let joint_travel_bounds = path.section_joint_travel_bounds(index);
            let mut check = |jaw| {
                self.check_continuous_section(
                    &mut path,
                    &mut reference,
                    index,
                    plan.peak_speed,
                    &joint_travel_bounds,
                    jaw,
                )
                .with_context(|| format!("Continuous section {index}"))
            };
            check(jaw_range.map(|range| range.0))?;
            if let Some((minimum, maximum)) = jaw_range
                && maximum > minimum
            {
                for jaw_fraction in crate::robot::path_sample_fractions(&[minimum], &[maximum]) {
                    check(Some(minimum + jaw_fraction * (maximum - minimum)))?;
                }
            }
        }
        Ok(())
    }

    fn check_continuous_section(
        &self,
        path: &mut crate::corner_path::ContinuousPath,
        reference: &mut crate::reference_filter::Reference,
        index: usize,
        peak_speed: f64,
        joint_travel_bounds: &[f64],
        jaw: Option<f64>,
    ) -> anyhow::Result<()> {
        // SAFETY: Speed times duration bounds joint travel even when the solver
        // temporarily reverses a joint between waypoints.
        let (start, end) = path.section_bounds(index);
        let extent = peak_speed * (end - start);
        if let Some(collision) = &self.self_collision
            && let Some((_, reason)) =
                collision.swept_prefix(joint_travel_bounds, jaw, |fraction, joints| {
                    path.sample(start + fraction * (end - start), reference);
                    joints.copy_from_slice(&reference.positions);
                })?
        {
            anyhow::bail!("{reason}");
        }
        if let Some(surface) = &self.surface {
            path.sample(start, reference);
            let mut previous = reference.positions.clone();
            for fraction in crate::robot::path_sample_fractions(&[0.0], &[extent]) {
                path.sample(start + fraction * (end - start), reference);
                if let Some(reason) = surface
                    .safe_prefix(&previous, &reference.positions, jaw)?
                    .refusal()
                {
                    anyhow::bail!("{reason}");
                }
                previous.clone_from(&reference.positions);
            }
        }
        Ok(())
    }

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

    fn require_joint_shape(&self, joints: &[f64]) -> Result<(), Rejection> {
        let expected = self.joint_names.len();
        if joints.len() != expected {
            return Err(rejection(
                RejectionReason::InvalidTarget,
                format!("Expected {expected} joints, got {}", joints.len()),
            ));
        }
        if !joints.iter().all(|value| value.is_finite()) {
            return Err(rejection(
                RejectionReason::InvalidTarget,
                "Joint positions must be finite",
            ));
        }
        Ok(())
    }

    fn joint_target(&self, joints: &[f64]) -> Result<Vec<f64>, Rejection> {
        self.require_joint_shape(joints)?;
        Ok(joints
            .iter()
            .zip(self.lower.iter().zip(&self.upper))
            .map(|(value, (lower, upper))| value.clamp(*lower, *upper))
            .collect())
    }

    fn solve_waypoint(
        &self,
        index: usize,
        previous: &[f64],
        position: [f64; 3],
        quaternion: [f64; 4],
    ) -> Result<(Vec<f64>, Option<PoseLimited>), Rejection> {
        let solved = self
            .solver
            .solve(position, OrientationGoal::Required { quaternion }, previous)
            .map_err(|why| rejection(RejectionReason::InvalidTarget, why.to_string()))?;
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
                    return Err(rejection(
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
                        advice: limited_summary(reason, &best, &advice),
                    }),
                ))
            }
        }
    }

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

    fn refine_clear_pose(
        &self,
        start: &[f64],
        solved: &[f64],
        position: [f64; 3],
        quaternion: [f64; 4],
        gripper_angle: Option<f64>,
    ) -> Option<Vec<f64>> {
        let refined = self
            .solver
            .refine_pose(position, quaternion, solved)
            .ok()??;
        matches!(
            self.path_check(start, &refined.joints, gripper_angle),
            Ok(PathCheck::Clear)
        )
        .then_some(refined.joints)
    }

    fn clear_prefix(
        start: &[f64],
        goal: Vec<f64>,
        path: anyhow::Result<PathCheck>,
    ) -> Result<(Vec<f64>, Option<PathLimited>), Rejection> {
        match path {
            Ok(PathCheck::Clear) => Ok((goal, None)),
            Ok(PathCheck::StopsAt { last_safe, reason }) if last_safe.as_slice() != start => {
                Ok((last_safe, Some(PathLimited { reason })))
            }
            Ok(PathCheck::StartRefused { reason } | PathCheck::StopsAt { reason, .. }) => {
                Err(rejection(RejectionReason::InvalidTarget, reason))
            }
            Err(error) => Err(rejection(
                RejectionReason::InvalidTarget,
                format!("{error:#}"),
            )),
        }
    }

    fn tool_rotation(&self, joints: &[f64]) -> UnitQuaternion<f64> {
        self.solver.chain().tool_pose(joints).rotation
    }

    fn projected(
        &self,
        joints: Vec<f64>,
        goal_position: [f64; 3],
        requested: Option<&UnitQuaternion<f64>>,
    ) -> ProjectedPose {
        let tool = self.solver.chain().tool_pose(&joints);
        ProjectedPose {
            position: tool.translation.vector.into(),
            quaternion: xyzw(&tool.rotation),
            orientation: orientation::tool_orientation(&tool.rotation, self.jaw_axis),
            position_error_meters: (tool.translation.vector - Vector3::from(goal_position)).norm(),
            orientation_error_radians: requested.map(|wanted| tool.rotation.angle_to(wanted)),
            joints,
        }
    }

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

fn segment(
    label: String,
    from: Vec<f64>,
    to: Vec<f64>,
    destination_position: Option<[f64; 3]>,
    destination_quaternion: Option<[f64; 4]>,
) -> Segment {
    Segment {
        final_gripper_angle: None,
        pass_through: None,
        label,
        corners: vec![from, to],
        minimum_duration_seconds: None,
        destination_position,
        destination_quaternion,
    }
}

fn shortfall_cause(reason: IkFailure) -> &'static str {
    match reason {
        IkFailure::AtJointLimit => "a joint stops at its limit short of the pose",
        IkFailure::Stalled => "the search stalled short of the pose",
        IkFailure::IterationLimit => "the search ran out of iterations",
        IkFailure::OutOfReach => "the pose is beyond the arm's reach",
    }
}

// WHY: Models ignored bare advice; stating the shortfall and where the arm goes instead,
// in the millimetres and degrees they work in, first makes the advice read as its fix.
fn limited_summary(reason: IkFailure, best: &IkPose, advice: &str) -> String {
    let [x, y, z] = best.tool_position.map(|meters| meters * 1000.0);
    format!(
        "The solver did not reach the target: the closest pose it found is {:.1} mm and {:.1} \
         degrees away ({}), with the tool at [{x:.1}, {y:.1}, {z:.1}] mm, and the arm moves \
         there instead. {advice}",
        best.position_error_meters * 1000.0,
        best.orientation_error_radians.to_degrees(),
        shortfall_cause(reason),
    )
}

fn not_reached(reason: IkFailure, best: &IkPose) -> String {
    format!(
        "No joints move the tool toward the pose: {}; the closest is {:.1} mm and {:.1} degrees \
         away; arm has not moved",
        shortfall_cause(reason),
        best.position_error_meters * 1000.0,
        best.orientation_error_radians.to_degrees()
    )
}

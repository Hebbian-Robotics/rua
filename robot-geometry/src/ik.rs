//! Inverse kinematics for a serial arm: joints that put the tool at a pose.
//!
//! A damped least-squares (Levenberg-Marquardt) iteration on the tool's position
//! and orientation error, with a weak pull toward the seed so the answer stays
//! near where the arm is. Every iterate stays inside the joint limits: each step
//! solves a bounded least-squares problem by active set, so a joint that reaches
//! its limit stops there and the others keep working, and nothing is clamped
//! after the fact. The solver reads the arm's own forward kinematics
//! ([`crate::robot::Robot`]), so it and the guards agree on where the tool is.

use nalgebra::{DMatrix, DVector, Isometry3, Quaternion, UnitQuaternion, Vector3};

use crate::joint_limits::{OutsideRecovery, clamp_within_recovery};
use crate::kinematics::SerialChain;
use crate::outcomes::{IkFailure, IkPose, IkResult, OrientationGoal};

/// A solution is at the target when the tool is this close to it.
pub const POSITION_TOLERANCE_METERS: f64 = 0.0005;
/// When orientation is required, a solution is this close to it.
pub const ORIENTATION_TOLERANCE_RADIANS: f64 = 0.005;

const MAXIMUM_ITERATIONS: usize = 300;
/// Starts spread through the joint ranges, tried after the seed and the middle.
const RESTARTS: usize = 24;
const RESTART_SEQUENCE_SEED: u64 = 0x5EED_1DE0_0000_0001;
/// The most any joint moves in one iteration.
const MAXIMUM_STEP_RADIANS: f64 = 0.3;
/// How hard the solution is pulled toward the seed, against a task error of
/// one metre or radian.
const POSTURE_WEIGHT: f64 = 1e-8;
/// With the orientation left free, how much the held orientation counts against
/// position: enough to break ties between solutions, never to give up position.
const FREE_ORIENTATION_WEIGHT: f64 = 0.01;
const INITIAL_DAMPING: f64 = 1e-3;
const MINIMUM_DAMPING: f64 = 1e-9;
const DAMPING_GROWTH: f64 = 4.0;
const DAMPING_DECAY: f64 = 3.0;
const TRIALS_PER_ITERATION: usize = 12;
/// A joint this close to a limit counts as at it.
const LIMIT_EPSILON: f64 = 1e-9;
/// Joints within this of a limit at the end of a stalled run are called at it.
const AT_LIMIT_RADIANS: f64 = 1e-6;
const UNIT_QUATERNION_TOLERANCE: f64 = 1e-5;
/// A step that changes the cost by less than this is no progress.
const PROGRESS_FLOOR: f64 = 1e-16;
/// The restarts stop once this many starts have fallen short at the same best
/// error: the search keeps finding one optimum.
const CONVERGED_STARTS: usize = 3;
/// Two starts found the same optimum when their task errors (metres, or
/// weighted radians) differ by less than this.
const SAME_ERROR: f64 = 1e-5;

/// A request the solver will not start on.
#[derive(Debug, Clone, PartialEq)]
pub enum IkRefusal {
    WrongJointCount {
        expected: usize,
        got: usize,
    },
    NotFinite,
    SeedOutsideLimits {
        joint: usize,
        value: f64,
        lower: f64,
        upper: f64,
        tolerance: f64,
    },
    NotAUnitQuaternion,
}

impl std::fmt::Display for IkRefusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WrongJointCount { expected, got } => {
                write!(formatter, "Expected {expected} joints, got {got}")
            }
            Self::NotFinite => write!(formatter, "Positions and joints must be finite"),
            Self::SeedOutsideLimits {
                joint,
                value,
                lower,
                upper,
                tolerance,
            } => write!(
                formatter,
                "Seed joint {joint} is at {value:.4} rad; its limits are {lower:.4} to \
                 {upper:.4} rad (starting-pose tolerance {tolerance:.4} rad)"
            ),
            Self::NotAUnitQuaternion => write!(formatter, "The quaternion must be normalized"),
        }
    }
}

impl std::error::Error for IkRefusal {}

/// The solver for one arm: its chain and joint limits.
#[derive(Debug, Clone)]
pub struct IkSolver {
    chain: SerialChain,
    lower: DVector<f64>,
    upper: DVector<f64>,
    seed_tolerance_radians: f64,
}

/// What a solve is after.
struct Goal {
    position: Vector3<f64>,
    rotation: UnitQuaternion<f64>,
    /// Weighs orientation error against position error in the cost.
    orientation_weight: f64,
    /// Whether the orientation must be within tolerance for success.
    orientation_required: bool,
}

/// One pose the search visited.
struct Visit {
    joints: DVector<f64>,
    tool: Isometry3<f64>,
    position_error: f64,
    orientation_error: f64,
}

enum Attempt {
    Reached(Visit),
    Fell { reason: IkFailure, best: Visit },
}

impl IkSolver {
    /// # Errors
    /// The limits do not match the chain or are not finite with `lower < upper`,
    /// or the seed tolerance is not a finite, non-negative number.
    pub fn new(
        chain: SerialChain,
        lower: &[f64],
        upper: &[f64],
        seed_tolerance_radians: f64,
    ) -> anyhow::Result<Self> {
        let count = chain.joint_count();
        anyhow::ensure!(
            lower.len() == count && upper.len() == count,
            "The chain has {count} joints but {} lower and {} upper limits",
            lower.len(),
            upper.len()
        );
        anyhow::ensure!(
            lower
                .iter()
                .zip(upper)
                .all(|(lower, upper)| lower.is_finite() && upper.is_finite() && lower < upper),
            "Joint limits must be finite with lower below upper"
        );
        anyhow::ensure!(
            seed_tolerance_radians.is_finite() && seed_tolerance_radians >= 0.0,
            "The seed tolerance must be finite and not negative"
        );
        Ok(Self {
            chain,
            lower: DVector::from_column_slice(lower),
            upper: DVector::from_column_slice(upper),
            seed_tolerance_radians,
        })
    }

    #[must_use]
    pub fn chain(&self) -> &SerialChain {
        &self.chain
    }

    /// Joints that put the tool at `position` with `orientation`, searching from
    /// `seed`: first as given, then once more from the middle of the joint
    /// ranges, which reaches targets in another elbow or wrist branch.
    ///
    /// # Errors
    /// The seed or the target is malformed, or the seed is outside the limits by
    /// more than the starting-pose tolerance (inside it, the seed is clamped in).
    pub fn solve(
        &self,
        position: [f64; 3],
        orientation: OrientationGoal,
        seed: &[f64],
    ) -> Result<IkResult, IkRefusal> {
        let seed = self.checked_seed(seed)?;
        if !position.iter().all(|value| value.is_finite()) {
            return Err(IkRefusal::NotFinite);
        }
        let held = self.chain.tool_pose(seed.as_slice()).rotation;
        let position = Vector3::from(position);
        // With the orientation free, the held orientation is a preference that can
        // trap the search in a pose that is far from the target; if every start
        // falls short with it, the same starts run once more with no preference.
        let goals = match orientation {
            OrientationGoal::Required { quaternion } => vec![Goal {
                position,
                rotation: required_rotation(quaternion)?,
                orientation_weight: 1.0,
                orientation_required: true,
            }],
            OrientationGoal::Free => [FREE_ORIENTATION_WEIGHT, 0.0]
                .into_iter()
                .map(|orientation_weight| Goal {
                    position,
                    rotation: held,
                    orientation_weight,
                    orientation_required: false,
                })
                .collect(),
        };
        // Beyond the arm's reach no start can succeed: the seed's and the middle's
        // searches stretch the arm toward the target, which is the closest pose.
        let out_of_reach = self
            .chain
            .out_of_reach(&position, POSITION_TOLERANCE_METERS);
        let start_count = if out_of_reach { 2 } else { usize::MAX };
        let mut best: Option<(IkFailure, Visit)> = None;
        for goal in &goals {
            let mut converged = ConvergedStarts::default();
            for start in self.starts(&seed).take(start_count) {
                match self.attempt(goal, start, &seed) {
                    Attempt::Reached(visit) => return Ok(IkResult::Solved(Self::pose(&visit))),
                    Attempt::Fell {
                        reason,
                        best: visit,
                    } => {
                        let settled = converged.settled(task_error(goal, &visit));
                        if best
                            .as_ref()
                            .is_none_or(|(_, held)| closer(&goals[0], &visit, held))
                        {
                            best = Some((reason, visit));
                        }
                        if settled {
                            break;
                        }
                    }
                }
            }
        }
        let (reason, visit) = best.expect("two attempts ran");
        Ok(IkResult::Failed {
            reason: if out_of_reach {
                IkFailure::OutOfReach
            } else {
                reason
            },
            best: Self::pose(&visit),
        })
    }

    /// Where the search starts, in order: the seed, the middle of the joint
    /// ranges (another elbow or wrist branch than the seed's), then points spread
    /// through the ranges by a fixed sequence. A later start runs only when every
    /// earlier one fell short, so a target near the seed never leaves it.
    fn starts(&self, seed: &DVector<f64>) -> impl Iterator<Item = DVector<f64>> + '_ {
        let middle = (&self.lower + &self.upper) / 2.0;
        let mut state = RESTART_SEQUENCE_SEED;
        let spread = (0..RESTARTS).map(move |_| {
            DVector::from_fn(self.lower.len(), |joint, _| {
                let fraction = next_fraction(&mut state);
                self.lower[joint] + (self.upper[joint] - self.lower[joint]) * fraction
            })
        });
        [seed.clone(), middle].into_iter().chain(spread)
    }

    fn checked_seed(&self, seed: &[f64]) -> Result<DVector<f64>, IkRefusal> {
        let expected = self.chain.joint_count();
        if seed.len() != expected {
            return Err(IkRefusal::WrongJointCount {
                expected,
                got: seed.len(),
            });
        }
        if !seed.iter().all(|value| value.is_finite()) {
            return Err(IkRefusal::NotFinite);
        }
        clamp_within_recovery(
            seed,
            self.lower.as_slice(),
            self.upper.as_slice(),
            self.seed_tolerance_radians,
        )
        .map(DVector::from_vec)
        .map_err(
            |OutsideRecovery { joint, value }| IkRefusal::SeedOutsideLimits {
                joint,
                value,
                lower: self.lower[joint],
                upper: self.upper[joint],
                tolerance: self.seed_tolerance_radians,
            },
        )
    }

    fn pose(visit: &Visit) -> IkPose {
        let position = visit.tool.translation.vector;
        let rotation = visit.tool.rotation;
        IkPose {
            joints: visit.joints.iter().copied().collect(),
            tool_position: [position.x, position.y, position.z],
            tool_quaternion: [rotation.i, rotation.j, rotation.k, rotation.w],
            position_error_meters: visit.position_error,
            orientation_error_radians: visit.orientation_error,
        }
    }

    fn visit(&self, goal: &Goal, joints: DVector<f64>) -> Visit {
        let tool = self.chain.tool_pose(joints.as_slice());
        let (position, rotation) = residual(goal, &tool);
        Visit {
            joints,
            tool,
            position_error: position.norm(),
            orientation_error: rotation.norm(),
        }
    }

    /// The cost the iteration minimizes: task error plus the pull toward the seed.
    fn cost(goal: &Goal, visit: &Visit, seed: &DVector<f64>) -> f64 {
        let weighted = goal.orientation_weight * visit.orientation_error;
        let task = visit.position_error.powi(2) + weighted.powi(2);
        f64::midpoint(task, POSTURE_WEIGHT * (&visit.joints - seed).norm_squared())
    }

    fn reached(goal: &Goal, visit: &Visit) -> bool {
        visit.position_error < POSITION_TOLERANCE_METERS
            && (!goal.orientation_required
                || visit.orientation_error < ORIENTATION_TOLERANCE_RADIANS)
    }

    fn attempt(&self, goal: &Goal, start: DVector<f64>, seed: &DVector<f64>) -> Attempt {
        let mut current = self.visit(goal, start);
        let mut best = Visit {
            joints: current.joints.clone(),
            tool: current.tool,
            position_error: current.position_error,
            orientation_error: current.orientation_error,
        };
        let mut damping = INITIAL_DAMPING;
        for _ in 0..MAXIMUM_ITERATIONS {
            if Self::reached(goal, &current) {
                return Attempt::Reached(current);
            }
            if closer(goal, &current, &best) {
                best = Visit {
                    joints: current.joints.clone(),
                    tool: current.tool,
                    position_error: current.position_error,
                    orientation_error: current.orientation_error,
                };
            }
            let frames = self.chain.frames(current.joints.as_slice());
            let (position_residual, rotation_residual) = residual(goal, &frames.tool);
            let mut jacobian = self.chain.jacobian(&frames);
            for column in 0..jacobian.ncols() {
                for row in 3..6 {
                    jacobian[(row, column)] *= goal.orientation_weight;
                }
            }
            let mut error = DVector::zeros(6);
            error.fixed_rows_mut::<3>(0).copy_from(&position_residual);
            error
                .fixed_rows_mut::<3>(3)
                .copy_from(&(rotation_residual * goal.orientation_weight));
            let count = jacobian.ncols();
            let hessian =
                jacobian.tr_mul(&jacobian) + DMatrix::identity(count, count) * POSTURE_WEIGHT;
            let gradient = jacobian.tr_mul(&error) - (&current.joints - seed) * POSTURE_WEIGHT;
            let cost = Self::cost(goal, &current, seed);

            let mut improved = None;
            for _ in 0..TRIALS_PER_ITERATION {
                let step = self.bounded_step(&hessian, &gradient, damping, &current.joints);
                let trial = self.visit(goal, self.trial(&current.joints, &step));
                if Self::cost(goal, &trial, seed) < cost - PROGRESS_FLOOR {
                    improved = Some(trial);
                    damping = (damping / DAMPING_DECAY).max(MINIMUM_DAMPING);
                    break;
                }
                damping *= DAMPING_GROWTH;
            }
            if let Some(next) = improved {
                current = next;
            } else {
                let at_limit = (0..count).any(|joint| {
                    best.joints[joint] - self.lower[joint] < AT_LIMIT_RADIANS
                        || self.upper[joint] - best.joints[joint] < AT_LIMIT_RADIANS
                });
                let reason = if at_limit {
                    IkFailure::AtJointLimit
                } else {
                    IkFailure::Stalled
                };
                return Attempt::Fell { reason, best };
            }
        }
        if Self::reached(goal, &current) {
            return Attempt::Reached(current);
        }
        if closer(goal, &current, &best) {
            best = current;
        }
        Attempt::Fell {
            reason: IkFailure::IterationLimit,
            best,
        }
    }

    /// The joint change that best reduces the cost for this damping while every
    /// joint stays inside its limits, at most `MAXIMUM_STEP_RADIANS` in any joint.
    ///
    /// Active set: a joint at a limit that the unconstrained step pushes outward
    /// starts pinned; the rest are solved together, and any that would leave
    /// their range are pinned at the limit and the rest solved again.
    /// The joints a step leads to.
    fn trial(&self, joints: &DVector<f64>, step: &DVector<f64>) -> DVector<f64> {
        // A step pinned to a limit is `limit - joint`, and adding it back can
        // round a unit past the limit; the clamp lands it on the limit.
        let mut trial = joints + step;
        for joint in 0..trial.len() {
            trial[joint] = trial[joint].clamp(self.lower[joint], self.upper[joint]);
        }
        trial
    }

    fn bounded_step(
        &self,
        hessian: &DMatrix<f64>,
        gradient: &DVector<f64>,
        damping: f64,
        joints: &DVector<f64>,
    ) -> DVector<f64> {
        let count = joints.len();
        let mut pinned: Vec<Option<f64>> = (0..count)
            .map(|joint| {
                let at_lower = joints[joint] - self.lower[joint] <= LIMIT_EPSILON;
                let at_upper = self.upper[joint] - joints[joint] <= LIMIT_EPSILON;
                ((at_lower && gradient[joint] < 0.0) || (at_upper && gradient[joint] > 0.0))
                    .then_some(0.0)
            })
            .collect();
        let mut step = DVector::zeros(count);
        for _ in 0..=count {
            step = solve_free(hessian, gradient, damping, &pinned);
            let mut violated = false;
            for joint in 0..count {
                if pinned[joint].is_some() {
                    continue;
                }
                let target = joints[joint] + step[joint];
                if target < self.lower[joint] {
                    pinned[joint] = Some(self.lower[joint] - joints[joint]);
                    violated = true;
                } else if target > self.upper[joint] {
                    pinned[joint] = Some(self.upper[joint] - joints[joint]);
                    violated = true;
                }
            }
            if !violated {
                break;
            }
        }
        let largest = step.amax();
        if largest > MAXIMUM_STEP_RADIANS {
            step *= MAXIMUM_STEP_RADIANS / largest;
        }
        step
    }
}

/// The next number in `[0, 1)` of a fixed pseudo-random sequence (SplitMix64):
/// the same restarts every time, with no random-number crate.
fn next_fraction(state: &mut u64) -> f64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut mixed = *state;
    mixed = (mixed ^ (mixed >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    mixed = (mixed ^ (mixed >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    mixed ^= mixed >> 31;
    f64::from(u32::try_from(mixed >> 40).expect("24-bit value fits in u32"))
        / f64::from(1_u32 << 24)
}

/// How far a pose is from the goal: position and weighted orientation error,
/// without the seed's pull.
fn task_error(goal: &Goal, visit: &Visit) -> f64 {
    (visit.position_error.powi(2) + (goal.orientation_weight * visit.orientation_error).powi(2))
        .sqrt()
}

/// Whether `visit` is nearer the goal than `held`, by the task error alone (the
/// seed's pull does not count when choosing which pose to report).
fn closer(goal: &Goal, visit: &Visit, held: &Visit) -> bool {
    task_error(goal, visit) < task_error(goal, held)
}

/// Whether the restarts keep landing on the same best pose, so more of them
/// would only find it again.
#[derive(Default)]
struct ConvergedStarts {
    best_error: Option<f64>,
    agreeing: usize,
}

impl ConvergedStarts {
    /// Count a start that fell short at `error`; true once enough agree on the best.
    fn settled(&mut self, error: f64) -> bool {
        match self.best_error {
            Some(best) if (error - best).abs() < SAME_ERROR => self.agreeing += 1,
            Some(best) if error > best => {}
            _ => {
                self.best_error = Some(error);
                self.agreeing = 1;
            }
        }
        self.agreeing >= CONVERGED_STARTS
    }
}

/// The task error: the position and rotation vector that take the tool to the goal.
fn residual(goal: &Goal, tool: &Isometry3<f64>) -> (Vector3<f64>, Vector3<f64>) {
    (
        goal.position - tool.translation.vector,
        (goal.rotation * tool.rotation.inverse()).scaled_axis(),
    )
}

fn required_rotation(quaternion: [f64; 4]) -> Result<UnitQuaternion<f64>, IkRefusal> {
    if !quaternion.iter().all(|value| value.is_finite()) {
        return Err(IkRefusal::NotFinite);
    }
    let norm = quaternion
        .iter()
        .map(|value| value * value)
        .sum::<f64>()
        .sqrt();
    if (norm - 1.0).abs() > UNIT_QUATERNION_TOLERANCE {
        return Err(IkRefusal::NotAUnitQuaternion);
    }
    let [x, y, z, w] = quaternion;
    Ok(UnitQuaternion::from_quaternion(Quaternion::new(w, x, y, z)))
}

/// Solve `(H + damping I) step = gradient` over the joints that are not pinned,
/// holding the pinned ones at their given change. A system that cannot be solved
/// leaves the free joints still.
fn solve_free(
    hessian: &DMatrix<f64>,
    gradient: &DVector<f64>,
    damping: f64,
    pinned: &[Option<f64>],
) -> DVector<f64> {
    let count = pinned.len();
    let free: Vec<usize> = (0..count)
        .filter(|joint| pinned[*joint].is_none())
        .collect();
    let mut step = DVector::from_fn(count, |joint, _| pinned[joint].unwrap_or(0.0));
    if free.is_empty() {
        return step;
    }
    let size = free.len();
    let matrix = DMatrix::from_fn(size, size, |row, column| {
        hessian[(free[row], free[column])] + if row == column { damping } else { 0.0 }
    });
    let right = DVector::from_fn(size, |row, _| {
        let joint = free[row];
        let pulled: f64 = (0..count)
            .filter_map(|other| pinned[other].map(|change| hessian[(joint, other)] * change))
            .sum();
        gradient[joint] - pulled
    });
    if let Some(factor) = matrix.cholesky() {
        let solution = factor.solve(&right);
        for (row, joint) in free.iter().enumerate() {
            step[*joint] = solution[row];
        }
    }
    step
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kinematics::ChainJoint;
    use nalgebra::Unit;

    /// A deterministic spread of numbers in `[0, 1)`, so the tests need no
    /// random-number crate and never change between runs.
    struct Sequence(u64);

    impl Sequence {
        fn next(&mut self) -> f64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            f64::from(u32::try_from(self.0 >> 40).expect("24-bit value fits in u32"))
                / f64::from(1_u32 << 24)
        }

        fn between(&mut self, lower: f64, upper: f64) -> f64 {
            lower + (upper - lower) * self.next()
        }
    }

    fn translate(x: f64, y: f64, z: f64) -> Isometry3<f64> {
        Isometry3::translation(x, y, z)
    }

    fn joint(before: Isometry3<f64>, axis: [f64; 3]) -> ChainJoint {
        ChainJoint {
            before,
            axis: Unit::new_normalize(Vector3::from(axis)),
        }
    }

    /// Seven revolute joints about z, each a metre from the last: the chain the
    /// station's kinematics test reaches an analytic pose with.
    fn planar_chain() -> IkSolver {
        let joints = (0..7)
            .map(|_| joint(translate(1.0, 0.0, 0.0), [0.0, 0.0, 1.0]))
            .collect();
        IkSolver::new(
            SerialChain::new(joints, Isometry3::identity()),
            &[-1.0; 7],
            &[1.0; 7],
            0.01,
        )
        .unwrap()
    }

    /// An SO-101-shaped arm: pan about z, lift, elbow and wrist flex about y,
    /// roll about the forearm's x axis, with the tool ahead of the wrist.
    fn so101_like() -> IkSolver {
        let joints = vec![
            joint(translate(0.0, 0.0, 0.1), [0.0, 0.0, 1.0]),
            joint(translate(0.03, 0.0, 0.06), [0.0, 1.0, 0.0]),
            joint(translate(0.115, 0.0, 0.03), [0.0, 1.0, 0.0]),
            joint(translate(0.135, 0.0, 0.0), [0.0, 1.0, 0.0]),
            joint(translate(0.06, 0.0, 0.0), [1.0, 0.0, 0.0]),
        ];
        IkSolver::new(
            SerialChain::new(joints, translate(0.1, 0.0, 0.0)),
            &[-1.9, -1.75, -1.7, -1.65, -2.75],
            &[1.9, 1.75, 1.55, 1.65, 2.85],
            0.01,
        )
        .unwrap()
    }

    fn solved(result: Result<IkResult, IkRefusal>) -> IkPose {
        match result.unwrap() {
            IkResult::Solved(pose) => pose,
            IkResult::Failed { reason, best } => {
                panic!("not solved ({reason:?}): {best:?}")
            }
        }
    }

    fn required(solver: &IkSolver, joints: &[f64]) -> ([f64; 3], OrientationGoal) {
        let tool = solver.chain.tool_pose(joints);
        let rotation = tool.rotation;
        (
            [
                tool.translation.vector.x,
                tool.translation.vector.y,
                tool.translation.vector.z,
            ],
            OrientationGoal::Required {
                quaternion: [rotation.i, rotation.j, rotation.k, rotation.w],
            },
        )
    }

    #[test]
    fn the_analytic_planar_pose_is_reached_within_half_a_millimetre() {
        let solver = planar_chain();
        let angle = 0.05_f64;
        let target = [1.0 + 6.0 * angle.cos(), 6.0 * angle.sin(), 0.0];
        let quaternion = [0.0, 0.0, (angle / 2.0).sin(), (angle / 2.0).cos()];
        let pose =
            solved(solver.solve(target, OrientationGoal::Required { quaternion }, &[0.0; 7]));
        assert!(pose.position_error_meters < POSITION_TOLERANCE_METERS);
        assert!(pose.orientation_error_radians < ORIENTATION_TOLERANCE_RADIANS);
        assert!(pose.joints.iter().all(|angle| (-1.0..=1.0).contains(angle)));
    }

    #[test]
    fn a_placed_arm_solves_poses_in_the_world_frame_it_is_placed_in() {
        let base = Isometry3::from_parts(
            nalgebra::Translation3::new(0.4, -0.2, 0.05),
            UnitQuaternion::from_euler_angles(0.0, 0.0, 0.7),
        );
        let unplaced = so101_like();
        let placed = IkSolver {
            chain: unplaced.chain.clone().placed_at(base),
            ..unplaced.clone()
        };
        let goal = [0.3, -0.4, 0.6, 0.2, 0.5];
        let world_tool = base * unplaced.chain.tool_pose(&goal);
        let rotation = world_tool.rotation;
        let pose = solved(placed.solve(
            world_tool.translation.vector.into(),
            OrientationGoal::Required {
                quaternion: [rotation.i, rotation.j, rotation.k, rotation.w],
            },
            &[0.0; 5],
        ));
        let reached = base * unplaced.chain.tool_pose(&pose.joints);
        assert!(
            (reached.translation.vector - world_tool.translation.vector).norm()
                < POSITION_TOLERANCE_METERS
        );
    }

    #[test]
    fn a_swing_across_the_whole_roll_range_converges() {
        let solver = so101_like();
        let start = [0.0, -0.5, 0.5, 0.0, -2.65];
        let goal = [0.0, -0.5, 0.5, 0.0, 2.75];
        let (position, orientation) = required(&solver, &goal);
        let pose = solved(solver.solve(position, orientation, &start));
        assert!(pose.position_error_meters < POSITION_TOLERANCE_METERS);
        assert!(pose.orientation_error_radians < ORIENTATION_TOLERANCE_RADIANS);
    }

    #[test]
    fn the_planar_chain_swings_across_its_whole_range_too() {
        let solver = planar_chain();
        let mut start = [0.0; 7];
        start[3] = -0.9;
        let mut goal = [0.0; 7];
        goal[3] = 0.9;
        let (position, orientation) = required(&solver, &goal);
        let pose = solved(solver.solve(position, orientation, &start));
        assert!(pose.position_error_meters < POSITION_TOLERANCE_METERS);
    }

    #[test]
    fn nearby_targets_are_reached_from_the_seed_without_leaving_it_far_behind() {
        let solver = so101_like();
        let mut numbers = Sequence(7);
        let mut solved_count = 0;
        for _ in 0..300 {
            let goal: Vec<f64> = (0..5)
                .map(|joint| numbers.between(solver.lower[joint] + 0.1, solver.upper[joint] - 0.1))
                .collect();
            let seed: Vec<f64> = goal
                .iter()
                .enumerate()
                .map(|(joint, value)| {
                    (value + numbers.between(-0.5, 0.5))
                        .clamp(solver.lower[joint], solver.upper[joint])
                })
                .collect();
            let (position, orientation) = required(&solver, &goal);
            if let IkResult::Solved(pose) = solver.solve(position, orientation, &seed).unwrap() {
                solved_count += 1;
                assert!(pose.position_error_meters < POSITION_TOLERANCE_METERS);
                for ((found, asked), start) in pose.joints.iter().zip(&goal).zip(&seed) {
                    assert!(
                        (found - start).abs() <= 1.0 || (found - asked).abs() <= 1.0,
                        "a solution left the seed behind: {found} from {start}, goal {asked}"
                    );
                }
            }
        }
        assert!(solved_count >= 290, "only {solved_count} of 300 solved");
    }

    #[test]
    fn every_returned_joint_is_inside_its_limits() {
        let solver = so101_like();
        let mut numbers = Sequence(11);
        for _ in 0..200 {
            let target = [
                numbers.between(-0.4, 0.4),
                numbers.between(-0.4, 0.4),
                numbers.between(-0.1, 0.45),
            ];
            let result = solver
                .solve(target, OrientationGoal::Free, &[0.0, -0.5, 0.5, 0.0, 0.0])
                .unwrap();
            let pose = match result {
                IkResult::Solved(pose) | IkResult::Failed { best: pose, .. } => pose,
            };
            for (joint, value) in pose.joints.iter().enumerate() {
                assert!(*value >= solver.lower[joint] && *value <= solver.upper[joint]);
            }
        }
    }

    #[test]
    fn a_step_pinned_to_a_limit_lands_on_it_and_not_past_it() {
        // A step pinned to a limit is `limit - joint`, and `joint + (limit - joint)`
        // can round one unit past `limit`: here to 0.20326983630657217, from a
        // case where the solver handed the planner a joint past its limit.
        let upper = 0.203_269_836_306_572_14;
        let joint_value = -0.083_387_674_458_780_12;
        assert!(joint_value + (upper - joint_value) > upper);
        let joints = vec![joint(Isometry3::identity(), [0.0, 0.0, 1.0])];
        let solver = IkSolver::new(
            SerialChain::new(joints, Isometry3::identity()),
            &[-1.0],
            &[upper],
            0.0,
        )
        .unwrap();
        // A pull far past the limit, so the step is pinned to it.
        let hessian = DMatrix::from_element(1, 1, 1.0);
        let gradient = DVector::from_element(1, 10.0);
        let current = DVector::from_element(1, joint_value);
        let step = solver.bounded_step(&hessian, &gradient, 0.0, &current);
        assert_eq!(solver.trial(&current, &step)[0], upper);
    }

    #[test]
    fn a_pose_past_a_limit_fails_instead_of_being_squeezed_in() {
        let joints = vec![joint(Isometry3::identity(), [0.0, 0.0, 1.0])];
        let solver = IkSolver::new(
            SerialChain::new(joints, Isometry3::identity()),
            &[-1.0],
            &[1.0],
            0.01,
        )
        .unwrap();
        let angle = 1.05_f64;
        let orientation = OrientationGoal::Required {
            quaternion: [0.0, 0.0, (angle / 2.0).sin(), (angle / 2.0).cos()],
        };
        match solver.solve([0.0, 0.0, 0.0], orientation, &[0.0]).unwrap() {
            IkResult::Failed { reason, best } => {
                assert_eq!(reason, IkFailure::AtJointLimit);
                assert!((best.joints[0] - 1.0).abs() < 1e-6);
                assert!(best.orientation_error_radians > ORIENTATION_TOLERANCE_RADIANS);
            }
            IkResult::Solved(pose) => panic!("solved outside the limit: {pose:?}"),
        }
    }

    #[test]
    fn a_joint_at_its_limit_does_not_stall_the_others() {
        // Reaching a point the planar chain can only get to by bending the other
        // joints further, with the first joint held at its limit by the seed.
        let solver = planar_chain();
        let seed = [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let goal = [1.0, -0.4, 0.3, -0.2, 0.1, 0.0, 0.0];
        let (position, orientation) = required(&solver, &goal);
        let pose = solved(solver.solve(position, orientation, &seed));
        assert!(pose.position_error_meters < POSITION_TOLERANCE_METERS);
    }

    #[test]
    fn a_free_orientation_reaches_a_position_a_fixed_one_cannot() {
        let solver = so101_like();
        let seed = [0.0, -0.5, 0.8, 0.6, 0.0];
        // Straight down with the tool above the table is out of this arm's reach
        // here, while some other orientation gets the position.
        let target = [0.2, 0.0, 0.15];
        let down = OrientationGoal::Required {
            quaternion: [0.0, 1.0, 0.0, 0.0],
        };
        let fixed = solver.solve(target, down, &seed).unwrap();
        let free = solver.solve(target, OrientationGoal::Free, &seed).unwrap();
        let free_error = match &free {
            IkResult::Solved(pose) | IkResult::Failed { best: pose, .. } => {
                pose.position_error_meters
            }
        };
        let fixed_error = match &fixed {
            IkResult::Solved(pose) | IkResult::Failed { best: pose, .. } => {
                pose.position_error_meters
            }
        };
        assert!(free_error < 0.005, "free error {free_error}");
        assert!(free_error <= fixed_error + 1e-9);
    }

    #[test]
    fn an_unreachable_target_reports_the_closest_pose_found() {
        let solver = so101_like();
        let result = solver
            .solve([5.0, 0.0, 0.0], OrientationGoal::Free, &[0.0; 5])
            .unwrap();
        match result {
            IkResult::Failed { best, .. } => {
                assert!(best.position_error_meters > 4.0);
                assert!(best.position_error_meters < 5.0);
            }
            IkResult::Solved(pose) => panic!("reached 5 m: {pose:?}"),
        }
    }

    #[test]
    fn a_target_past_the_links_laid_end_to_end_is_out_of_reach_and_the_arm_stretches_toward_it() {
        // The planar chain's first joint is at x = 1 and six metre links follow it.
        let solver = planar_chain();
        let seed = [0.3, -0.2, 0.1, 0.0, 0.0, 0.0, 0.0];
        let inside = solver.solve([6.99, 0.0, 0.0], OrientationGoal::Free, &seed);
        assert!(matches!(inside, Ok(IkResult::Solved(_))), "{inside:?}");
        match solver.solve([7.5, 0.0, 0.0], OrientationGoal::Free, &seed) {
            Ok(IkResult::Failed { reason, best }) => {
                assert_eq!(reason, IkFailure::OutOfReach);
                assert!((best.position_error_meters - 0.5).abs() < 1e-3, "{best:?}");
            }
            other => panic!("reached past the arm: {other:?}"),
        }
    }

    #[test]
    fn a_seed_just_outside_a_limit_is_clamped_and_one_far_outside_is_refused() {
        let solver = so101_like();
        let (position, orientation) = required(&solver, &[0.0, -0.5, 0.5, 0.0, 0.0]);
        let near = [1.9 + 0.01, -0.5, 0.5, 0.0, 0.0];
        assert!(solver.solve(position, orientation, &near).is_ok());
        let far = [1.9 + 0.1, -0.5, 0.5, 0.0, 0.0];
        assert!(matches!(
            solver.solve(position, orientation, &far),
            Err(IkRefusal::SeedOutsideLimits { joint: 0, .. })
        ));
    }

    #[test]
    fn malformed_requests_are_refused_before_solving() {
        let solver = so101_like();
        let free = OrientationGoal::Free;
        assert_eq!(
            solver.solve([0.2, 0.0, 0.1], free, &[0.0; 4]),
            Err(IkRefusal::WrongJointCount {
                expected: 5,
                got: 4
            })
        );
        assert_eq!(
            solver.solve([0.2, 0.0, 0.1], free, &[0.0, f64::NAN, 0.0, 0.0, 0.0]),
            Err(IkRefusal::NotFinite)
        );
        assert_eq!(
            solver.solve([f64::INFINITY, 0.0, 0.1], free, &[0.0; 5]),
            Err(IkRefusal::NotFinite)
        );
        let tilted = OrientationGoal::Required {
            quaternion: [0.0, 0.0, 0.0, 2.0],
        };
        assert_eq!(
            solver.solve([0.2, 0.0, 0.1], tilted, &[0.0; 5]),
            Err(IkRefusal::NotAUnitQuaternion)
        );
    }

    #[test]
    fn a_solve_gives_the_same_answer_every_time() {
        let solver = so101_like();
        let (position, orientation) = required(&solver, &[0.3, -0.6, 0.9, 0.2, 0.4]);
        let first = solver.solve(position, orientation, &[0.0; 5]).unwrap();
        let second = solver.solve(position, orientation, &[0.0; 5]).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn the_jacobian_matches_finite_differences_of_the_tool_pose() {
        let solver = so101_like();
        let joints = DVector::from_column_slice(&[0.3, -0.6, 0.9, 0.2, 0.4]);
        let frames = solver.chain.frames(joints.as_slice());
        let jacobian = solver.chain.jacobian(&frames);
        let step = 1e-6;
        for column in 0..5 {
            let mut shifted = joints.clone();
            shifted[column] += step;
            let moved = solver.chain.frames(shifted.as_slice()).tool;
            let linear = (moved.translation.vector - frames.tool.translation.vector) / step;
            let angular = (moved.rotation * frames.tool.rotation.inverse()).scaled_axis() / step;
            for row in 0..3 {
                assert!((jacobian[(row, column)] - linear[row]).abs() < 1e-5);
                assert!((jacobian[(row + 3, column)] - angular[row]).abs() < 1e-5);
            }
        }
    }

    #[test]
    fn a_solve_is_fast() {
        let solver = so101_like();
        let (position, orientation) = required(&solver, &[0.3, -0.6, 0.9, 0.2, 0.4]);
        let started = std::time::Instant::now();
        for _ in 0..100 {
            let _ = solver.solve(position, orientation, &[0.0, -0.5, 0.5, 0.0, 0.0]);
        }
        // The build is optimized even in tests; a solve is far under 10 ms.
        assert!(started.elapsed() / 100 < std::time::Duration::from_millis(10));
    }
}

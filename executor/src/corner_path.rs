//! One online Ruckig generator follows each straight joint-space section.
//! Each section targets its real endpoint, preserving commanded velocity and
//! acceleration between cycles. The tracking guard, run every cycle by the
//! control loop, is the check that an arm commanded to move follows.

use anyhow::{Context, bail, ensure};
use rsruckig::prelude::{
    InputParameter, Ruckig, RuckigResult, Synchronization, ThrowErrorHandler, Trajectory,
};
use serde::Serialize;

use crate::config::MotionLimits;
use crate::limits::{Trip, max_joint_deviation};
use crate::reference_filter::Reference;
use hebbian_contracts::{Checkpoint, CornerPlan};

/// Corners closer than this are one corner.
const SAME_CORNER_RADIANS: f64 = 1e-12;
/// The plan's first corner and the arm must agree to this, as in the station.
pub(crate) const CONTINUITY_RADIANS: f64 = 1e-8;

/// Ruckig's constructor wants a control period; these generators only evaluate
/// `at_time` at the caller's interval, so the value never affects the output.
const UNUSED_PERIOD_SECONDS: f64 = 0.01;

/// The pose the plan starts from: its first corner.
///
/// # Errors
/// The plan has no corners.
pub(crate) fn plan_initial_corner(plan: &CornerPlan) -> anyhow::Result<&[f64]> {
    plan.segments
        .first()
        .and_then(|segment| segment.corners.first())
        .map(Vec::as_slice)
        .context("An empty motion path")
}

/// # Errors
/// The plan is empty, not continuous, or not finite, or its speed is not positive.
pub fn validate_plan(plan: &CornerPlan, joint_count: usize) -> anyhow::Result<()> {
    ensure!(!plan.segments.is_empty(), "An empty motion path");
    ensure!(
        plan.peak_speed.is_finite() && plan.peak_speed > 0.0,
        "Path speed must be positive"
    );
    let mut previous = plan_initial_corner(plan)?;
    for segment in &plan.segments {
        ensure!(
            segment.corners.len() >= 2
                && segment
                    .corners
                    .iter()
                    .all(|corner| corner.len() == joint_count
                        && corner.iter().all(|value| value.is_finite())),
            "Segment {} needs two or more finite corners of {joint_count} joints",
            segment.label
        );
        ensure!(
            max_joint_deviation(&segment.corners[0], previous) <= CONTINUITY_RADIANS,
            "Discontinuous motion segments"
        );
        ensure!(
            segment.minimum_duration_seconds.is_none() || segment.corners.len() == 2,
            "A minimum duration needs a single straight section"
        );
        previous = &segment.corners[segment.corners.len() - 1];
    }
    Ok(())
}

/// A motion plan checked for the joint count of the arm that will execute it.
struct MotionPlan(CornerPlan);

impl MotionPlan {
    fn new(plan: CornerPlan, joint_count: usize) -> anyhow::Result<Self> {
        validate_plan(&plan, joint_count)?;
        Ok(Self(plan))
    }
}

impl std::ops::Deref for MotionPlan {
    type Target = CornerPlan;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// Straight joint-space sections between corner waypoints, at rest at each.
struct SmoothTrajectory {
    /// Section `i` runs from `waypoints[i]` to `waypoints[i + 1]`.
    waypoints: Vec<Vec<f64>>,
}

impl SmoothTrajectory {
    fn new(
        points: &[Vec<f64>],
        speed: f64,
        acceleration: f64,
        jerk: f64,
        minimum_duration: Option<f64>,
    ) -> anyhow::Result<Self> {
        ensure!(
            [speed, acceleration, jerk]
                .iter()
                .all(|limit| limit.is_finite() && *limit > 0.0),
            "Trajectory limits must be positive finite numbers"
        );
        ensure!(
            points.len() >= 2
                && points
                    .iter()
                    .all(|point| point.iter().all(|v| v.is_finite())),
            "A trajectory needs at least two finite joint points"
        );
        let joint_count = points[0].len();
        ensure!(joint_count > 0, "A trajectory needs joints");
        ensure!(
            minimum_duration.is_none() || points.len() == 2,
            "A minimum duration needs a single straight section"
        );
        let mut waypoints: Vec<Vec<f64>> = vec![points[0].clone()];
        for pair in points.windows(2) {
            if max_joint_deviation(&pair[0], &pair[1]) > SAME_CORNER_RADIANS {
                waypoints.push(pair[1].clone());
            }
        }
        let mut generator =
            Ruckig::<0, ThrowErrorHandler>::new(Some(joint_count), UNUSED_PERIOD_SECONDS);
        // Each section is computed once here so a plan Ruckig cannot follow is
        // refused before it starts.
        for pair in waypoints.windows(2) {
            let mut input = InputParameter::<0>::new(Some(joint_count));
            for (joint, (start, end)) in pair[0].iter().zip(&pair[1]).enumerate() {
                input.current_position[joint] = *start;
                input.target_position[joint] = *end;
                input.max_velocity[joint] = speed;
                input.max_acceleration[joint] = acceleration;
                input.max_jerk[joint] = jerk;
            }
            // Phase synchronization keeps every joint on the straight line.
            input.synchronization = Synchronization::Phase;
            input.minimum_duration = minimum_duration;
            let mut trajectory = Trajectory::<0>::new(Some(joint_count));
            match generator.calculate(&input, &mut trajectory) {
                Ok(RuckigResult::Working | RuckigResult::Finished) => {}
                Ok(other) => bail!("Ruckig trajectory failed: {other:?}"),
                Err(error) => bail!("Ruckig rejected the trajectory: {error}"),
            }
            let section_duration = trajectory.get_duration();
            ensure!(
                section_duration.is_finite() && section_duration >= 0.0,
                "Invalid Ruckig trajectory duration"
            );
        }
        Ok(Self { waypoints })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    /// Holding a rest point until the supervisor confirms the arm settled there.
    Settling,
    Moving,
    Done,
}

/// One persistent scalar generator owns a collision-checked straight section.
/// The scalar is measured in radians of the largest-travelling joint, avoiding
/// ill-conditioned normalization as a target is approached.
pub(crate) struct OnlineSection {
    start: Vec<f64>,
    target: Vec<f64>,
    distance: f64,
    generator: Ruckig<0, ThrowErrorHandler>,
    input: InputParameter<0>,
    trajectory: Trajectory<0>,
    positions: rsruckig::prelude::DataArrayOrVec<f64, 0>,
    velocities: rsruckig::prelude::DataArrayOrVec<f64, 0>,
    accelerations: rsruckig::prelude::DataArrayOrVec<f64, 0>,
}

impl OnlineSection {
    pub(crate) fn new(
        reference: &Reference,
        target: &[f64],
        speed: f64,
        limits: &MotionLimits,
    ) -> Self {
        let (joint, distance) = reference
            .positions
            .iter()
            .zip(target)
            .enumerate()
            .map(|(joint, (start, end))| (joint, (end - start).abs()))
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .expect("a path has joints");
        let direction = (target[joint] - reference.positions[joint]).signum();
        let mut input = InputParameter::<0>::new(Some(1));
        input.max_velocity[0] = speed;
        input.max_acceleration[0] = limits.acceleration_limit;
        input.max_jerk[0] = limits.jerk_limit;
        input.current_velocity[0] = reference.velocities[joint] * direction;
        input.current_acceleration[0] = reference.accelerations[joint] * direction;
        input.synchronization = Synchronization::None;
        Self {
            start: reference.positions.clone(),
            target: target.to_vec(),
            distance,
            generator: Ruckig::new(Some(1), UNUSED_PERIOD_SECONDS),
            input,
            trajectory: Trajectory::new(Some(1)),
            positions: rsruckig::prelude::DataArrayOrVec::new(Some(1), 0.0),
            velocities: rsruckig::prelude::DataArrayOrVec::new(Some(1), 0.0),
            accelerations: rsruckig::prelude::DataArrayOrVec::new(Some(1), 0.0),
        }
    }

    pub(crate) fn step(
        &mut self,
        interval: f64,
        minimum_duration: Option<f64>,
    ) -> anyhow::Result<Reference> {
        // Only the actual corner is a zero-velocity target. An intermediate
        // target would force Ruckig to keep braking before the real destination.
        self.input.target_position[0] = self.distance;
        self.input.minimum_duration = minimum_duration;
        match self.generator.calculate(&self.input, &mut self.trajectory) {
            Ok(RuckigResult::Working | RuckigResult::Finished) => {}
            Ok(other) => bail!("Online trajectory failed: {other:?}"),
            Err(error) => bail!("Online trajectory failed: {error}"),
        }
        self.trajectory.at_time(
            interval.min(self.trajectory.get_duration()),
            &mut Some(&mut self.positions),
            &mut Some(&mut self.velocities),
            &mut Some(&mut self.accelerations),
            &mut None,
            &mut None,
        );
        self.input.current_position[0] = self.positions[0];
        self.input.current_velocity[0] = self.velocities[0];
        self.input.current_acceleration[0] = self.accelerations[0];
        let mapped = |scalar: f64| {
            self.start
                .iter()
                .zip(&self.target)
                .map(|(start, end)| scalar / self.distance * (end - start))
                .collect::<Vec<_>>()
        };
        let positions = self
            .start
            .iter()
            .zip(mapped(self.positions[0]))
            .map(|(start, offset)| start + offset)
            .collect();
        Ok(Reference {
            positions,
            velocities: mapped(self.velocities[0]),
            accelerations: mapped(self.accelerations[0]),
            speed_limit: None,
        })
    }
}

struct CornerPathTracker {
    plan: MotionPlan,
    limits: MotionLimits,
    stage: Stage,
    reference: Reference,
    segment_index: usize,
    trajectories: Vec<SmoothTrajectory>,
    corner_index: usize,
    section: Option<OnlineSection>,
    segment_elapsed: f64,
    checkpoints: Vec<Checkpoint>,
    /// The last rest point was accepted outside tolerance after the settle timeout.
    stopped_short: bool,
}

impl CornerPathTracker {
    fn new(plan: MotionPlan, limits: MotionLimits) -> anyhow::Result<Self> {
        let trajectories = plan
            .segments
            .iter()
            .map(|segment| {
                SmoothTrajectory::new(
                    &segment.corners,
                    plan.peak_speed,
                    limits.acceleration_limit,
                    limits.jerk_limit,
                    segment.minimum_duration_seconds,
                )
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        let reference = Reference::at_rest(plan_initial_corner(&plan)?, None);
        Ok(Self {
            plan,
            limits,
            stage: Stage::Moving,
            reference,
            segment_index: 0,
            trajectories,
            corner_index: 1,
            section: None,
            segment_elapsed: 0.0,
            checkpoints: Vec::new(),
            stopped_short: false,
        })
    }

    fn segment_label(&self) -> Option<&str> {
        (self.stage != Stage::Done).then(|| self.plan.segments[self.segment_index].label.as_str())
    }

    fn advance(&mut self, interval: f64) -> anyhow::Result<()> {
        let corners = &self.trajectories[self.segment_index].waypoints;
        if corners.len() <= 1 {
            self.reference = Reference::at_rest(&corners[0], None);
            self.stage = Stage::Settling;
            return Ok(());
        }
        let target = &corners[self.corner_index];
        if max_joint_deviation(&self.reference.positions, target) <= SAME_CORNER_RADIANS
            && self.section.is_none()
        {
            self.reference = Reference::at_rest(target, None);
        } else {
            let section = self.section.get_or_insert_with(|| {
                OnlineSection::new(&self.reference, target, self.plan.peak_speed, &self.limits)
            });
            let minimum_duration = self.plan.segments[self.segment_index]
                .minimum_duration_seconds
                .map(|minimum| (minimum - self.segment_elapsed).max(0.0));
            self.reference = section.step(interval, minimum_duration)?;
        }
        self.segment_elapsed += interval;
        if max_joint_deviation(&self.reference.positions, target) <= SAME_CORNER_RADIANS
            && self.reference.velocities.iter().all(|v| v.abs() < 1e-8)
        {
            if self.corner_index + 1 < corners.len() {
                self.corner_index += 1;
                self.section = None;
            } else {
                self.stage = Stage::Settling;
            }
        }
        Ok(())
    }

    fn settled(&mut self, measured: &[f64], stalled_short: bool) -> anyhow::Result<()> {
        ensure!(
            self.stage == Stage::Settling,
            "Only a settling tracker can settle"
        );
        let segment = &self.plan.segments[self.segment_index];
        self.checkpoints.push(Checkpoint {
            label: segment.label.clone(),
            measured_joints: measured.to_vec(),
            target_position: segment.target_position,
            target_quaternion: segment.target_quaternion,
        });
        self.segment_index += 1;
        if self.segment_index == self.plan.segments.len() {
            self.stopped_short = stalled_short;
            self.stage = Stage::Done;
            return Ok(());
        }
        self.stage = Stage::Moving;
        self.corner_index = 1;
        self.section = None;
        self.segment_elapsed = 0.0;
        Ok(())
    }
}

/// Feedback-aware path execution: each rest point settles, or is accepted where
/// the arm was not within tolerance after the settle timeout.
pub struct MotionSupervisor {
    tracker: CornerPathTracker,
    limits: MotionLimits,
    settled_seconds: f64,
    settling_seconds: f64,
}

impl MotionSupervisor {
    /// # Errors
    /// The plan is malformed or a trajectory cannot be computed.
    pub fn new(
        plan: CornerPlan,
        limits: &MotionLimits,
        joint_count: usize,
    ) -> anyhow::Result<Self> {
        let plan = MotionPlan::new(plan, joint_count)?;
        Ok(Self {
            tracker: CornerPathTracker::new(plan, limits.clone())?,
            limits: limits.clone(),
            settled_seconds: 0.0,
            settling_seconds: 0.0,
        })
    }

    #[must_use]
    pub fn done(&self) -> bool {
        self.tracker.stage == Stage::Done
    }

    /// The motion ended with the arm not within the settled tolerance of its
    /// final rest point after the settle timeout.
    #[must_use]
    pub fn stopped_short(&self) -> bool {
        self.tracker.stopped_short
    }

    /// The motion has finished its path and is only waiting for the arm to settle
    /// at its final rest point.
    #[must_use]
    pub fn settling_at_end(&self) -> bool {
        let tracker = &self.tracker;
        tracker.stage == Stage::Settling && tracker.segment_index + 1 == tracker.plan.segments.len()
    }

    #[must_use]
    pub fn stage(&self) -> Stage {
        self.tracker.stage
    }

    #[must_use]
    pub(crate) fn segment_label(&self) -> Option<&str> {
        self.tracker.segment_label()
    }

    #[must_use]
    pub fn reference(&self) -> &Reference {
        &self.tracker.reference
    }

    #[must_use]
    pub(crate) fn final_pose(&self) -> &[f64] {
        let last = &self.tracker.plan.segments[self.tracker.plan.segments.len() - 1];
        &last.corners[last.corners.len() - 1]
    }

    /// Take the checkpoints reached since the last call.
    pub(crate) fn take_new_checkpoints(&mut self, already_taken: usize) -> Vec<Checkpoint> {
        self.tracker.checkpoints[already_taken.min(self.tracker.checkpoints.len())..].to_vec()
    }

    fn restart_stage(&mut self) {
        self.settled_seconds = 0.0;
        self.settling_seconds = 0.0;
    }

    fn internal(error: &anyhow::Error) -> Trip {
        Trip(format!("{error:#}"))
    }

    /// The reference for this cycle, given the measured joints. Tracking error is
    /// the control loop's check, made every cycle whatever is running.
    ///
    /// # Errors
    /// An internal `Trip` when the plan cannot be followed.
    pub fn step(&mut self, measured: &[f64], interval: f64) -> Result<Reference, Trip> {
        if self.tracker.stage == Stage::Settling {
            let endpoint_error = max_joint_deviation(measured, &self.tracker.reference.positions);
            self.settled_seconds = if endpoint_error <= self.limits.settled_error_radians {
                self.settled_seconds + interval
            } else {
                0.0
            };
            // An arm that stops short (servo deadband, a load) carries on from
            // where it is once the settle timeout passes outside the tolerance;
            // the checkpoint records the measured joints.
            self.settling_seconds += interval;
            let stalled = self.settling_seconds >= self.limits.settle_timeout_seconds
                && endpoint_error > self.limits.settled_error_radians;
            if self.settled_seconds >= self.limits.settle_dwell_seconds || stalled {
                self.tracker
                    .settled(measured, stalled)
                    .map_err(|e| Self::internal(&e))?;
                self.restart_stage();
            }
        }
        if self.tracker.stage == Stage::Moving {
            let previous_corner = self.tracker.corner_index;
            self.tracker
                .advance(interval)
                .map_err(|e| Self::internal(&e))?;
            if self.tracker.stage != Stage::Moving || self.tracker.corner_index != previous_corner {
                self.restart_stage();
            }
        }
        Ok(self.tracker.reference.clone())
    }
}

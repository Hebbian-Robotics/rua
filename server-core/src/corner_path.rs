mod continuous;

pub(crate) use continuous::ContinuousPath;

use anyhow::{Context, bail, ensure};
use rsruckig::prelude::{
    InputParameter, OutputParameter, Ruckig, RuckigResult, Synchronization, ThrowErrorHandler,
    Trajectory,
};
use serde::Serialize;

use crate::config::MotionLimits;
use crate::limits::{Trip, max_joint_deviation};
use crate::reference_filter::Reference;
use hebbian_contracts::{Checkpoint, CornerPlan, Segment};

const SAME_CORNER_RADIANS: f64 = 1e-12;
pub(crate) const CONTINUITY_RADIANS: f64 = 1e-8;
pub(crate) const MIN_TRACKING_SPEED_FRACTION: f64 = 0.3;

// CONSTRAINT: Validation only calculates trajectories. Online sections replace this initial
// constructor period with the actual cycle interval before updating.
pub(crate) const INITIAL_PERIOD_SECONDS: f64 = 0.01;

pub(crate) fn plan_initial_corner(plan: &CornerPlan) -> anyhow::Result<&[f64]> {
    plan.segments
        .first()
        .and_then(|segment| segment.corners.first())
        .map(Vec::as_slice)
        .context("An empty motion path")
}

pub fn validate_plan(plan: &CornerPlan, joint_count: usize) -> anyhow::Result<()> {
    ensure!(joint_count > 0, "A motion path needs joints");
    ensure!(!plan.segments.is_empty(), "An empty motion path");
    ensure!(
        plan.peak_speed.is_finite() && plan.peak_speed > 0.0,
        "Path speed must be positive"
    );
    ensure!(
        plan.segments
            .iter()
            .enumerate()
            .all(|(index, segment)| segment.pass_through.is_some()
                == (plan.is_continuous() && index + 1 < plan.segments.len()))
            && (!plan.is_continuous()
                || plan
                    .segments
                    .iter()
                    .all(|segment| segment.corners.len() == 2)),
        "Invalid continuous path boundaries"
    );
    ensure!(
        plan.segments[..plan.segments.len() - 1]
            .iter()
            .all(|segment| segment.final_gripper_angle.is_none()),
        "A final gripper target belongs on the last segment"
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
        ensure!(
            segment
                .minimum_duration_seconds
                .is_none_or(|duration| duration.is_finite() && duration >= 0.0),
            "A minimum duration must be finite and not negative"
        );
        if !plan.is_continuous() {
            validate_straight_section_travel(segment, &segment.corners[0])?;
        }
        previous = &segment.corners[segment.corners.len() - 1];
    }
    Ok(())
}

fn validate_straight_section_travel(segment: &Segment, start: &[f64]) -> anyhow::Result<()> {
    ensure!(
        !segment
            .minimum_duration_seconds
            .is_some_and(|seconds| seconds > 0.0)
            || max_joint_deviation(start, &segment.corners[1]) > SAME_CORNER_RADIANS,
        "A positive minimum duration needs joint travel"
    );
    Ok(())
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ValidatedPlan(CornerPlan);

impl ValidatedPlan {
    pub(crate) fn new(plan: CornerPlan, joint_count: usize) -> anyhow::Result<Self> {
        validate_plan(&plan, joint_count)?;
        Ok(Self(plan))
    }

    pub(crate) fn initial_corner(&self) -> &[f64] {
        &self.0.segments[0].corners[0]
    }

    pub(crate) fn final_segment(&self) -> &Segment {
        &self.0.segments[self.0.segments.len() - 1]
    }

    pub(crate) fn final_corner(&self) -> &[f64] {
        let segment = self.final_segment();
        &segment.corners[segment.corners.len() - 1]
    }

    pub(crate) fn rebase(&mut self, initial: Vec<f64>) -> anyhow::Result<()> {
        ensure!(
            initial.len() == self.initial_corner().len()
                && initial.iter().all(|value| value.is_finite()),
            "A motion start must have the plan's joint count and finite positions"
        );
        ensure!(
            !self.0.is_continuous()
                || max_joint_deviation(self.initial_corner(), &initial) <= CONTINUITY_RADIANS,
            "Continuous path start changed after planning"
        );
        // INVARIANT: Preserve the exact input to the station's checked waypoint solver.
        if !self.0.is_continuous() {
            validate_straight_section_travel(&self.0.segments[0], &initial)?;
            self.0.segments[0].corners[0] = initial;
        }
        Ok(())
    }
}

impl std::ops::Deref for ValidatedPlan {
    type Target = CornerPlan;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

struct SmoothTrajectory {
    waypoints: Vec<Vec<f64>>,
    section_durations: Vec<f64>,
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
        // WHY: Use the same scalar trajectory as execution when setting deadlines.
        // A multidimensional phase request can fall back to a different path.
        let mut section_durations = Vec::with_capacity(waypoints.len() - 1);
        for pair in waypoints.windows(2) {
            let distance = max_joint_deviation(&pair[0], &pair[1]);
            let mut input = InputParameter::<1>::new(None);
            input.target_position[0] = distance;
            input.max_velocity[0] = speed;
            input.max_acceleration[0] = acceleration;
            input.max_jerk[0] = jerk;
            input.synchronization = Synchronization::Time;
            input.minimum_duration = minimum_duration;
            let mut trajectory = Trajectory::<1>::new(None);
            match Ruckig::<1, ThrowErrorHandler>::new(None, INITIAL_PERIOD_SECONDS)
                .calculate(&input, &mut trajectory)
            {
                Ok(RuckigResult::Working | RuckigResult::Finished) => {}
                Ok(other) => bail!("Ruckig trajectory failed: {other:?}"),
                Err(error) => bail!("Ruckig rejected the trajectory: {error}"),
            }
            let section_duration = trajectory.get_duration();
            ensure!(
                section_duration.is_finite() && section_duration >= 0.0,
                "Invalid Ruckig trajectory duration"
            );
            section_durations.push(section_duration);
        }
        Ok(Self {
            waypoints,
            section_durations,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Settling,
    Moving,
    Done,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum MotionEnd {
    Reached,
    StoppedShort,
    TrajectoryTimedOut,
}

// WHY: One persistent scalar generator owns a collision-checked straight section.
// The scalar is measured in radians of the largest-travelling joint, avoiding
// ill-conditioned normalization as a target is approached.
pub(crate) struct OnlineSection {
    start: Vec<f64>,
    target: Vec<f64>,
    distance: f64,
    generator: Ruckig<1, ThrowErrorHandler>,
    input: InputParameter<1>,
    output: OutputParameter<1>,
}

impl OnlineSection {
    pub(crate) fn new(
        reference: &Reference,
        target: &[f64],
        speed: f64,
        limits: &MotionLimits,
        minimum_duration: Option<f64>,
    ) -> anyhow::Result<Self> {
        let joint_count = target.len();
        ensure!(
            joint_count > 0
                && [
                    &reference.positions,
                    &reference.velocities,
                    &reference.accelerations
                ]
                .iter()
                .all(|values| values.len() == joint_count
                    && values.iter().all(|value| value.is_finite()))
                && target.iter().all(|value| value.is_finite()),
            "A section needs matching nonempty finite reference and target joints"
        );
        ensure!(
            [speed, limits.acceleration_limit, limits.jerk_limit]
                .iter()
                .all(|value| value.is_finite() && *value > 0.0),
            "A section needs positive finite motion limits"
        );
        ensure!(
            minimum_duration.is_none_or(|duration| duration.is_finite() && duration >= 0.0),
            "A section duration must be finite and not negative"
        );
        let (joint, distance) = reference
            .positions
            .iter()
            .zip(target)
            .enumerate()
            .map(|(joint, (start, end))| (joint, (end - start).abs()))
            .max_by(|left, right| left.1.total_cmp(&right.1))
            .context("A section needs joints")?;
        ensure!(
            distance.is_finite() && distance > 0.0,
            "A section needs positive finite travel"
        );
        let direction = (target[joint] - reference.positions[joint]).signum();
        let mut input = InputParameter::<1>::new(None);
        input.target_position[0] = distance;
        input.minimum_duration = minimum_duration;
        input.max_velocity[0] = speed;
        input.max_acceleration[0] = limits.acceleration_limit;
        input.max_jerk[0] = limits.jerk_limit;
        input.current_velocity[0] = reference.velocities[joint] * direction;
        input.current_acceleration[0] = reference.accelerations[joint] * direction;
        // CONSTRAINT: Even this one scalar must synchronize to a requested minimum duration.
        input.synchronization = Synchronization::Time;
        Ok(Self {
            start: reference.positions.clone(),
            target: target.to_vec(),
            distance,
            generator: Ruckig::new(None, INITIAL_PERIOD_SECONDS),
            input,
            output: OutputParameter::new(None),
        })
    }

    fn step_with_speed_limit(
        &mut self,
        interval: f64,
        reference: &mut Reference,
        speed_limit: f64,
    ) -> anyhow::Result<bool> {
        let timed = self.input.minimum_duration.is_some();
        if !timed {
            self.input.max_velocity[0] = speed_limit;
        }
        let finished = self.step(interval, reference)?;
        // ROBOT-SAFETY: Replanning a timed section would reset its duration. Trip
        // before sending a sample above the lag-adjusted speed cap.
        ensure!(
            !timed || self.output.new_velocity[0].abs() <= speed_limit + 1e-9,
            "Tracking lag requires slowing down a timed trajectory"
        );
        Ok(finished)
    }

    pub(crate) fn step(
        &mut self,
        interval: f64,
        reference: &mut Reference,
    ) -> anyhow::Result<bool> {
        // CONSTRAINT: The section owns a fixed endpoint and duration. Feed back Ruckig's
        // emitted state so it can keep sampling the same trajectory.
        ensure!(
            interval.is_finite() && interval > 0.0,
            "A section interval must be positive and finite"
        );
        self.generator.delta_time = interval;
        let finished = match self.generator.update(&self.input, &mut self.output) {
            Ok(RuckigResult::Working) => false,
            Ok(RuckigResult::Finished) => true,
            Ok(other) => bail!("Online trajectory failed: {other:?}"),
            Err(error) => bail!("Online trajectory failed: {error}"),
        };
        self.output.pass_to_input(&mut self.input);
        if finished {
            reference.set_at_rest(&self.target, None);
            return Ok(true);
        }
        reference.positions.resize(self.start.len(), 0.0);
        reference.velocities.resize(self.start.len(), 0.0);
        reference.accelerations.resize(self.start.len(), 0.0);
        for (joint, (start, target)) in self.start.iter().zip(&self.target).enumerate() {
            let displacement = target - start;
            reference.positions[joint] =
                start + self.output.new_position[0] / self.distance * displacement;
            reference.velocities[joint] =
                self.output.new_velocity[0] / self.distance * displacement;
            reference.accelerations[joint] =
                self.output.new_acceleration[0] / self.distance * displacement;
        }
        reference.speed_limit = None;
        Ok(false)
    }
}

pub struct MotionSupervisor {
    plan: ValidatedPlan,
    limits: MotionLimits,
    stage: Stage,
    reference: Reference,
    segment_index: usize,
    trajectories: Vec<SmoothTrajectory>,
    continuous_path: Option<ContinuousPath>,
    waypoint_feedback_lag_seconds: f64,
    section_elapsed: f64,
    pending_waypoint_seconds: f64,
    corner_index: usize,
    section: Option<OnlineSection>,
    checkpoints: Vec<Checkpoint>,
    end: Option<MotionEnd>,
    settled_seconds: f64,
    settling_seconds: f64,
}

impl MotionSupervisor {
    pub fn new(
        plan: CornerPlan,
        limits: &MotionLimits,
        joint_count: usize,
    ) -> anyhow::Result<Self> {
        Self::from_validated(ValidatedPlan::new(plan, joint_count)?, limits, 0.0, 0.0)
    }

    pub(crate) fn from_validated(
        plan: ValidatedPlan,
        limits: &MotionLimits,
        waypoint_feedback_lag_seconds: f64,
        maximum_step_seconds: f64,
    ) -> anyhow::Result<Self> {
        let mut limits = limits.clone();
        if let Some(trajectory_limits) = plan.trajectory_limits {
            limits.acceleration_limit = trajectory_limits.acceleration_limit;
            limits.jerk_limit = trajectory_limits.jerk_limit;
        }
        let continuous_path = plan
            .is_continuous()
            .then(|| ContinuousPath::new(&plan, &limits, maximum_step_seconds))
            .transpose()?;
        let trajectories = if continuous_path.is_some() {
            Vec::new()
        } else {
            plan.segments
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
                .collect::<anyhow::Result<_>>()?
        };
        let reference = Reference::at_rest(plan.initial_corner(), None);
        Ok(Self {
            plan,
            limits,
            stage: Stage::Moving,
            reference,
            segment_index: 0,
            trajectories,
            continuous_path,
            waypoint_feedback_lag_seconds,
            section_elapsed: 0.0,
            pending_waypoint_seconds: 0.0,
            corner_index: 1,
            section: None,
            checkpoints: Vec::new(),
            end: None,
            settled_seconds: 0.0,
            settling_seconds: 0.0,
        })
    }

    #[must_use]
    pub(crate) fn segment_label(&self) -> Option<&str> {
        (self.stage != Stage::Done).then(|| self.plan.segments[self.segment_index].label.as_str())
    }

    fn advance_path(
        &mut self,
        measured: &[f64],
        interval: f64,
        speed_fraction: f64,
    ) -> anyhow::Result<()> {
        if self.continuous_path.is_some() {
            return self.advance_continuous(measured, interval, speed_fraction);
        }
        let trajectory = &self.trajectories[self.segment_index];
        let corners = &trajectory.waypoints;
        if corners.len() <= 1 {
            self.reference.set_at_rest(&corners[0], None);
            self.stage = Stage::Settling;
            return Ok(());
        }
        let target = &corners[self.corner_index];
        let finished = if max_joint_deviation(&self.reference.positions, target)
            <= SAME_CORNER_RADIANS
            && self.section.is_none()
        {
            self.reference.set_at_rest(target, None);
            true
        } else {
            if self.section.is_none() {
                self.section = Some(OnlineSection::new(
                    &self.reference,
                    target,
                    self.plan.peak_speed,
                    &self.limits,
                    self.plan.segments[self.segment_index].minimum_duration_seconds,
                )?);
            }
            self.section
                .as_mut()
                .context("Missing the active trajectory section")?
                .step_with_speed_limit(
                    interval,
                    &mut self.reference,
                    self.plan.peak_speed * speed_fraction,
                )?
        };
        self.section_elapsed += interval;
        // WHY: Retiming can restart the generator's clock. Bound the total time
        // spent on this section, even if a late cycle samples the endpoint.
        let deadline = trajectory.section_durations[self.corner_index - 1]
            / MIN_TRACKING_SPEED_FRACTION
            + self.limits.settle_timeout_seconds;
        if self.section_elapsed >= deadline {
            self.end = Some(MotionEnd::TrajectoryTimedOut);
            self.stage = Stage::Done;
        } else if finished {
            if self.corner_index + 1 < corners.len() {
                self.corner_index += 1;
                self.section = None;
                self.section_elapsed = 0.0;
            } else {
                self.stage = Stage::Settling;
            }
        }
        Ok(())
    }

    fn advance_continuous(
        &mut self,
        measured: &[f64],
        interval: f64,
        speed_fraction: f64,
    ) -> anyhow::Result<()> {
        let path = self
            .continuous_path
            .as_mut()
            .context("Missing continuous path")?;
        let trajectory_seconds = path.advance_clock(interval, speed_fraction)?;
        let (section_start, section_end) = path.section_bounds(self.segment_index);
        let duration = section_end - section_start;
        ensure!(
            interval.is_finite() && interval > 0.0 && interval < duration,
            "Controller interval exceeds the continuous section duration"
        );
        let crossing = trajectory_seconds >= section_end;
        if crossing && self.segment_index + 1 < self.plan.segments.len() {
            let (_, next_end) = path.section_bounds(self.segment_index + 1);
            ensure!(
                trajectory_seconds < next_end,
                "Controller interval crosses more than one continuous section boundary"
            );
        }

        // ROBOT-SAFETY: Feedback can arrive after the reference crosses. Keep the position
        // tolerance and confirm each waypoint before crossing another boundary.
        let unconfirmed = self.checkpoints.len();
        let had_pending_crossing = unconfirmed < self.segment_index;
        if had_pending_crossing {
            self.pending_waypoint_seconds += interval;
            ensure!(
                self.pending_waypoint_seconds <= self.waypoint_feedback_lag_seconds
                    && (self.confirm_waypoint(unconfirmed, measured) || !crossing),
                "Arm missed pass-through waypoint {unconfirmed}"
            );
        }

        self.section_elapsed = trajectory_seconds - section_start;
        if crossing {
            let index = self.segment_index;
            if index + 1 == self.plan.segments.len() {
                self.reference
                    .set_at_rest(&self.plan.segments[index].corners[1], None);
                self.stage = Stage::Settling;
                return Ok(());
            }
            // INVARIANT: At most one checkpoint per write, including its jaw command.
            if !had_pending_crossing && !self.confirm_waypoint(index, measured) {
                ensure!(
                    self.waypoint_feedback_lag_seconds > 0.0,
                    "Arm missed pass-through waypoint {index}"
                );
            }
            self.segment_index += 1;
            self.section_elapsed -= duration;
            self.pending_waypoint_seconds = 0.0;
        }
        self.continuous_path
            .as_mut()
            .context("Missing continuous path")?
            .sample(trajectory_seconds, &mut self.reference);
        Ok(())
    }

    fn confirm_waypoint(&mut self, index: usize, measured: &[f64]) -> bool {
        if max_joint_deviation(measured, &self.plan.segments[index].corners[1])
            > self.limits.settled_error_radians
        {
            return false;
        }
        self.record_checkpoint(index, measured);
        true
    }

    fn record_checkpoint(&mut self, index: usize, measured: &[f64]) {
        let segment = &self.plan.segments[index];
        self.checkpoints.push(Checkpoint {
            label: segment.label.clone(),
            measured_joints: measured.to_vec(),
            destination_position: segment.destination_position,
            destination_quaternion: segment.destination_quaternion,
        });
    }

    fn settled(&mut self, measured: &[f64], stalled_short: bool) -> anyhow::Result<()> {
        ensure!(
            self.stage == Stage::Settling,
            "Only a settling tracker can settle"
        );
        self.record_checkpoint(self.segment_index, measured);
        self.segment_index += 1;
        if stalled_short || self.segment_index == self.plan.segments.len() {
            self.end = Some(if stalled_short {
                MotionEnd::StoppedShort
            } else {
                MotionEnd::Reached
            });
            self.stage = Stage::Done;
            return Ok(());
        }
        self.stage = Stage::Moving;
        self.corner_index = 1;
        self.section = None;
        self.section_elapsed = 0.0;
        Ok(())
    }

    #[must_use]
    pub fn done(&self) -> bool {
        self.end().is_some()
    }

    #[must_use]
    pub fn stopped_short(&self) -> bool {
        self.end() == Some(MotionEnd::StoppedShort)
    }

    pub(crate) fn end(&self) -> Option<MotionEnd> {
        self.end
    }

    #[must_use]
    pub fn settling_at_end(&self) -> bool {
        self.stage == Stage::Settling && self.segment_index + 1 == self.plan.segments.len()
    }

    #[must_use]
    pub fn stage(&self) -> Stage {
        self.stage
    }

    #[must_use]
    pub(crate) fn requested_joint_speed(&self) -> f64 {
        self.plan.peak_speed
    }

    #[must_use]
    pub(crate) fn planned_peak_joint_speed(&self) -> f64 {
        self.continuous_path
            .as_ref()
            .map_or(self.plan.peak_speed, |path| path.peak_joint_speed)
    }

    #[must_use]
    pub fn reference(&self) -> &Reference {
        &self.reference
    }

    pub(crate) fn copy_reference_to(&self, destination: &mut Reference) {
        destination.copy_from(&self.reference);
    }

    pub(crate) fn copy_joint_reference_to(
        &self,
        joints: std::ops::Range<usize>,
        destination: &mut Reference,
    ) {
        destination
            .positions
            .copy_from_slice(&self.reference.positions[joints.clone()]);
        destination
            .velocities
            .copy_from_slice(&self.reference.velocities[joints.clone()]);
        destination
            .accelerations
            .copy_from_slice(&self.reference.accelerations[joints]);
    }

    #[must_use]
    pub(crate) fn final_pose(&self) -> &[f64] {
        self.plan.final_corner()
    }

    pub(crate) fn final_gripper_angle(&self) -> Option<hebbian_contracts::JawAngle> {
        self.plan.final_segment().final_gripper_angle
    }

    pub(crate) fn waypoint_gripper(
        &self,
        checkpoint: usize,
    ) -> Option<hebbian_contracts::JawAngle> {
        self.plan
            .segments
            .get(checkpoint)?
            .pass_through?
            .gripper_angle
    }

    pub(crate) fn new_checkpoints(&self, already_taken: usize) -> &[Checkpoint] {
        &self.checkpoints[already_taken.min(self.checkpoints.len())..]
    }

    fn restart_stage(&mut self) {
        self.settled_seconds = 0.0;
        self.settling_seconds = 0.0;
    }

    fn internal(error: &anyhow::Error) -> Trip {
        Trip::Trajectory(format!("{error:#}"))
    }

    // ROBOT-SAFETY: Tracking remains the control loop’s per-cycle check regardless of which trajectory is running.
    pub fn step(&mut self, measured: &[f64], interval: f64) -> Result<Reference, Trip> {
        self.advance(measured, interval)?;
        Ok(self.reference.clone())
    }

    // WHY: Reuse the retained reference and caller buffers rather than allocating a snapshot every cycle.
    pub(crate) fn advance(&mut self, measured: &[f64], interval: f64) -> Result<(), Trip> {
        self.advance_with_speed_fraction(measured, interval, 1.0)
    }

    pub(crate) fn advance_with_speed_fraction(
        &mut self,
        measured: &[f64],
        interval: f64,
        speed_fraction: f64,
    ) -> Result<(), Trip> {
        if self.stage == Stage::Settling {
            let endpoint_error = max_joint_deviation(measured, &self.reference.positions);
            self.settled_seconds = if endpoint_error <= self.limits.settled_error_radians {
                self.settled_seconds + interval
            } else {
                0.0
            };
            // WHY: Briefly entering tolerance at the deadline cannot extend settling.
            self.settling_seconds += interval;
            let settled = self.settled_seconds >= self.limits.settle_dwell_seconds;
            if settled || self.settling_seconds >= self.limits.settle_timeout_seconds {
                self.settled(measured, !settled)
                    .map_err(|e| Self::internal(&e))?;
                self.restart_stage();
            }
        }
        if self.stage == Stage::Moving {
            let previous_corner = self.corner_index;
            self.advance_path(measured, interval, speed_fraction)
                .map_err(|e| Self::internal(&e))?;
            if self.stage != Stage::Moving || self.corner_index != previous_corner {
                self.restart_stage();
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use hebbian_contracts::TrajectoryLimits;

    use super::*;
    use crate::config::{ControllerConfig, TEST_CONFIG};

    fn planned_move(
        knots: &[(&str, f64)],
        trajectory_limits: Option<TrajectoryLimits>,
    ) -> MotionSupervisor {
        let mut input = ControllerConfig::parse(TEST_CONFIG).unwrap().into_input();
        input.frequency_hertz = 50.0;
        input.tracking_following_lag_seconds = 0.1;
        let config = ControllerConfig::try_from(input).unwrap();
        let segments = knots
            .windows(2)
            .enumerate()
            .map(|(index, pair)| Segment {
                label: pair[1].0.to_owned(),
                corners: vec![vec![pair[0].1], vec![pair[1].1]],
                pass_through: (index + 2 < knots.len()).then_some(hebbian_contracts::PassThrough {
                    gripper_angle: None,
                }),
                minimum_duration_seconds: None,
                final_gripper_angle: None,
                destination_position: None,
                destination_quaternion: None,
            })
            .collect();
        let plan = ValidatedPlan::new(
            CornerPlan {
                segments,
                peak_speed: 0.6,
                trajectory_limits,
            },
            1,
        )
        .unwrap();
        MotionSupervisor::from_validated(
            plan,
            &config.motion,
            config.tracking_following_lag_seconds,
            config.control_timing.maximum_step_seconds,
        )
        .unwrap()
    }

    const STANDARD_KNOTS: [(&str, f64); 3] = [("start", 0.0), ("waypoint", 0.3), ("goal", 0.6)];

    fn measure_move_duration(
        mut motion: MotionSupervisor,
        limits: TrajectoryLimits,
        goal: f64,
    ) -> usize {
        const INTERVAL_SECONDS: f64 = 0.01;
        const SAMPLE_TOLERANCE: f64 = 1e-6;
        let mut measured = motion.reference().positions[0];
        let mut previous_acceleration = motion.reference().accelerations[0];
        for sample_count in 1..=1_000 {
            let reference = motion.step(&[measured], INTERVAL_SECONDS).unwrap();
            measured = reference.positions[0];
            let acceleration = reference.accelerations[0];
            let sampled_jerk = (acceleration - previous_acceleration) / INTERVAL_SECONDS;
            assert!(
                acceleration.abs() <= limits.acceleration_limit + SAMPLE_TOLERANCE,
                "Sample {sample_count} exceeded the acceleration cap: {acceleration}"
            );
            assert!(
                sampled_jerk.abs() <= limits.jerk_limit + SAMPLE_TOLERANCE,
                "Sample {sample_count} exceeded the jerk cap: {sampled_jerk}"
            );
            previous_acceleration = acceleration;
            if motion.settling_at_end() {
                assert!((measured - goal).abs() <= SAMPLE_TOLERANCE);
                return sample_count;
            }
        }
        panic!("The move did not reach its goal");
    }

    fn assert_motion_caps_slow_the_move(knots: &[(&str, f64)]) {
        let goal = knots.last().unwrap().1;
        let profile_limits = ControllerConfig::parse(TEST_CONFIG)
            .unwrap()
            .motion
            .trajectory_limits();
        let baseline_duration =
            measure_move_duration(planned_move(knots, None), profile_limits, goal);
        for limits in [
            TrajectoryLimits {
                acceleration_limit: 0.5,
                ..profile_limits
            },
            TrajectoryLimits {
                jerk_limit: 1.0,
                ..profile_limits
            },
        ] {
            let capped_duration =
                measure_move_duration(planned_move(knots, Some(limits)), limits, goal);
            assert!(
                capped_duration > baseline_duration,
                "The move should take longer with {limits:?}"
            );
        }
    }

    #[test]
    fn straight_move_samples_respect_acceleration_and_jerk_caps() {
        assert_motion_caps_slow_the_move(&[("start", 0.0), ("goal", 0.6)]);
    }

    #[test]
    fn pass_through_move_samples_respect_acceleration_and_jerk_caps() {
        assert_motion_caps_slow_the_move(&STANDARD_KNOTS);
    }

    #[test]
    fn continuous_move_smoothly_slows_and_recovers_without_missing_waypoints() {
        let mut motion = planned_move(&STANDARD_KNOTS, None);
        let limits = ControllerConfig::parse(TEST_CONFIG)
            .unwrap()
            .motion
            .trajectory_limits();
        let mut measured = [0.0];
        let mut previous_acceleration = 0.0;
        let mut slowed_samples = 0;
        for sample in 0..2_000 {
            let speed_fraction = if (25..65).contains(&sample) { 0.3 } else { 1.0 };
            motion
                .advance_with_speed_fraction(&measured, 0.01, speed_fraction)
                .unwrap();
            let reference = motion.reference();
            let jerk = (reference.accelerations[0] - previous_acceleration) / 0.01;
            assert!(reference.accelerations[0].abs() <= limits.acceleration_limit + 1e-6);
            assert!(
                jerk.abs() <= limits.jerk_limit + 1e-6,
                "sample {sample}, jerk {jerk}"
            );
            previous_acceleration = reference.accelerations[0];
            measured[0] = reference.positions[0];
            if speed_fraction < 1.0 {
                slowed_samples += 1;
            }
            if motion.done() {
                break;
            }
        }
        assert!(slowed_samples > 0);
        assert!(matches!(motion.end(), Some(MotionEnd::Reached)));
        let labels: Vec<&str> = motion
            .new_checkpoints(0)
            .iter()
            .map(|point| point.label.as_str())
            .collect();
        assert_eq!(labels, ["waypoint", "goal"]);
    }

    #[test]
    fn changing_tracking_requests_preserves_continuous_motion_limits() {
        let mut motion = planned_move(&STANDARD_KNOTS, None);
        let limits = ControllerConfig::parse(TEST_CONFIG)
            .unwrap()
            .motion
            .trajectory_limits();
        let mut measured = [0.0];
        let mut previous_acceleration = 0.0;
        for sample in 0..2_000 {
            let speed_fraction = if sample % 8 < 4 { 0.3 } else { 1.0 };
            motion
                .advance_with_speed_fraction(&measured, 0.01, speed_fraction)
                .unwrap();
            let reference = motion.reference();
            let jerk = (reference.accelerations[0] - previous_acceleration) / 0.01;
            assert!(reference.accelerations[0].abs() <= limits.acceleration_limit + 1e-6);
            assert!(
                jerk.abs() <= limits.jerk_limit + 1e-6,
                "sample {sample}, jerk {jerk}"
            );
            previous_acceleration = reference.accelerations[0];
            measured[0] = reference.positions[0];
            if motion.done() {
                break;
            }
        }
        assert!(matches!(motion.end(), Some(MotionEnd::Reached)));
    }

    #[test]
    fn so101_circle_remains_smooth_when_tracking_requests_slowdown() {
        let knots: Vec<Vec<f64>> =
            serde_json::from_str(include_str!("../tests/support/so101_circle_waypoints.json"))
                .unwrap();
        let segments = knots
            .windows(2)
            .enumerate()
            .map(|(index, pair)| Segment {
                label: format!("waypoint_{index}"),
                corners: pair.to_vec(),
                pass_through: (index + 2 < knots.len()).then_some(hebbian_contracts::PassThrough {
                    gripper_angle: None,
                }),
                minimum_duration_seconds: None,
                final_gripper_angle: None,
                destination_position: None,
                destination_quaternion: None,
            })
            .collect();
        let limits = ControllerConfig::parse(TEST_CONFIG).unwrap().motion.clone();
        let mut motion = MotionSupervisor::new(
            CornerPlan {
                segments,
                peak_speed: 2.5,
                trajectory_limits: None,
            },
            &limits,
            knots[0].len(),
        )
        .unwrap();
        let mut measured = knots[0].clone();
        let mut previous_accelerations = vec![0.0; measured.len()];
        for sample in 0..5_000 {
            let speed_fraction = if (80..180).contains(&sample) {
                0.3
            } else {
                1.0
            };
            motion
                .advance_with_speed_fraction(&measured, 0.01, speed_fraction)
                .unwrap();
            let reference = motion.reference();
            for (joint, previous) in previous_accelerations.iter_mut().enumerate() {
                let acceleration = reference.accelerations[joint];
                let jerk = (acceleration - *previous) / 0.01;
                assert!(acceleration.abs() <= limits.acceleration_limit + 1e-6);
                assert!(
                    jerk.abs() <= limits.jerk_limit + 1e-6,
                    "sample {sample}, joint {joint}, jerk {jerk}"
                );
                *previous = acceleration;
            }
            measured.clone_from(&reference.positions);
            if motion.done() {
                break;
            }
        }
        assert!(matches!(motion.end(), Some(MotionEnd::Reached)));
        assert_eq!(motion.new_checkpoints(0).len(), knots.len() - 1);
    }

    #[test]
    fn slowing_down_preserves_waypoint_confirmation_with_delayed_feedback() {
        let mut motion = planned_move(&STANDARD_KNOTS, None);
        let mut feedback_history = VecDeque::from([[0.0]; 7]);
        for sample in 0..2_000 {
            let measured = feedback_history.pop_front().unwrap();
            let fraction = if (25..100).contains(&sample) {
                0.5
            } else {
                1.0
            };
            motion
                .advance_with_speed_fraction(&measured, 0.01, fraction)
                .unwrap();
            feedback_history.push_back([motion.reference().positions[0]]);
            if motion.done() {
                break;
            }
        }
        assert!(matches!(motion.end(), Some(MotionEnd::Reached)));
        assert_eq!(motion.new_checkpoints(0).len(), 2);
    }

    #[test]
    fn dense_waypoints_still_report_every_crossing_with_late_control_cycles() {
        let mut motion = planned_move(
            &[
                ("start", 0.0),
                ("first", 0.3),
                ("second", 0.300_005),
                ("goal", 0.6),
            ],
            None,
        );
        let mut measured = [0.0];
        for _ in 0..200 {
            let reference = motion.step(&measured, 0.04).unwrap();
            measured[0] = reference.positions[0];
            if motion.done() {
                break;
            }
        }
        assert!(matches!(motion.end(), Some(MotionEnd::Reached)));
        let labels: Vec<&str> = motion
            .new_checkpoints(0)
            .iter()
            .map(|point| point.label.as_str())
            .collect();
        assert_eq!(labels, ["first", "second", "goal"]);
    }

    #[test]
    fn pass_through_checkpoint_waits_for_measured_crossing() {
        let mut motion = planned_move(&STANDARD_KNOTS, None);
        let mut feedback_history = VecDeque::from([[0.0]; 7]);
        let mut reference_crossed_before_feedback = false;
        for _ in 0..200 {
            let measured = feedback_history.pop_front().unwrap();
            let reference = motion.step(&measured, 0.01).unwrap();
            feedback_history.push_back([reference.positions[0]]);
            if reference.positions[0] >= 0.3 && motion.new_checkpoints(0).is_empty() {
                reference_crossed_before_feedback = true;
            }
            if motion.done() {
                break;
            }
        }
        assert!(reference_crossed_before_feedback);
        assert!(matches!(motion.end(), Some(MotionEnd::Reached)));
        let [waypoint, goal] = motion.new_checkpoints(0) else {
            panic!("Expected one measured waypoint and the final goal");
        };
        assert_eq!(waypoint.label, "waypoint");
        assert!((waypoint.measured_joints[0] - 0.3).abs() <= 0.02);
        assert_eq!(goal.label, "goal");
    }

    #[test]
    fn pass_through_trips_when_feedback_never_reaches_tolerance() {
        let mut motion = planned_move(&STANDARD_KNOTS, None);
        let failure = (0..200).find_map(|_| motion.step(&[0.0], 0.01).err());
        assert!(matches!(
            failure,
            Some(Trip::Trajectory(detail)) if detail == "Arm missed pass-through waypoint 0"
        ));
        assert_eq!(motion.new_checkpoints(0), []);
        assert!(motion.reference().positions[0] < 0.6);
    }
}

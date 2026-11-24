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
const INITIAL_PERIOD_SECONDS: f64 = 0.01;

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
                    .all(|segment| segment.corners.len() == 2
                        && segment.minimum_duration_seconds.is_none())),
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
        previous = &segment.corners[segment.corners.len() - 1];
    }
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
        self.0.segments[0].corners[0] = initial;
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
        let mut generator =
            Ruckig::<0, ThrowErrorHandler>::new(Some(joint_count), INITIAL_PERIOD_SECONDS);
        // WHY: Each section is computed once here so a plan Ruckig cannot follow is
        // refused before it starts.
        let mut section_durations = Vec::with_capacity(waypoints.len() - 1);
        for pair in waypoints.windows(2) {
            let mut input = InputParameter::<0>::new(Some(joint_count));
            for (joint, (start, end)) in pair[0].iter().zip(&pair[1]).enumerate() {
                input.current_position[joint] = *start;
                input.target_position[joint] = *end;
                input.max_velocity[joint] = speed;
                input.max_acceleration[joint] = acceleration;
                input.max_jerk[joint] = jerk;
            }
            // WHY: Phase synchronization keeps every joint on the straight line.
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
        // SAFETY: Replanning a timed section would reset its duration. Trip
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

struct CornerPathTracker {
    plan: ValidatedPlan,
    limits: MotionLimits,
    stage: Stage,
    reference: Reference,
    segment_index: usize,
    trajectories: Vec<SmoothTrajectory>,
    continuous_path: Option<crate::continuous_path::ContinuousPath>,
    section_elapsed: f64,
    corner_index: usize,
    section: Option<OnlineSection>,
    checkpoints: Vec<Checkpoint>,
    end: Option<MotionEnd>,
}

impl CornerPathTracker {
    fn new(plan: ValidatedPlan, limits: MotionLimits) -> anyhow::Result<Self> {
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
        let continuous_path = plan
            .is_continuous()
            .then(|| crate::continuous_path::ContinuousPath::new(&plan, &limits))
            .transpose()?;
        let reference = Reference::at_rest(plan.initial_corner(), None);
        Ok(Self {
            plan,
            limits,
            stage: Stage::Moving,
            reference,
            segment_index: 0,
            trajectories,
            continuous_path,
            section_elapsed: 0.0,
            corner_index: 1,
            section: None,
            checkpoints: Vec::new(),
            end: None,
        })
    }

    fn segment_label(&self) -> Option<&str> {
        (self.stage != Stage::Done).then(|| self.plan.segments[self.segment_index].label.as_str())
    }

    fn advance(
        &mut self,
        measured: &[f64],
        interval: f64,
        speed_fraction: f64,
    ) -> anyhow::Result<()> {
        if let Some(path) = &self.continuous_path {
            // SAFETY: A changing velocity cap needs a new time parameterization. Stop a
            // continuous move when tracking calls for slowing down.
            ensure!(
                speed_fraction >= 1.0,
                "Continuous motion exceeded its tracking margin"
            );
            let duration = path.section_seconds();
            ensure!(
                interval.is_finite() && interval > 0.0 && interval < duration,
                "Controller interval exceeds the continuous section duration"
            );
            self.section_elapsed += interval;
            if self.section_elapsed >= duration {
                let segment = &self.plan.segments[self.segment_index];
                if self.segment_index + 1 == self.plan.segments.len() {
                    self.reference.set_at_rest(&segment.corners[1], None);
                    self.stage = Stage::Settling;
                    return Ok(());
                }
                ensure!(
                    max_joint_deviation(measured, &segment.corners[1])
                        <= self.limits.settled_error_radians,
                    "Arm missed pass-through waypoint {}",
                    self.segment_index
                );
                self.checkpoints.push(Checkpoint {
                    label: segment.label.clone(),
                    measured_joints: measured.to_vec(),
                    destination_position: segment.destination_position,
                    destination_quaternion: segment.destination_quaternion,
                });
                self.segment_index += 1;
                self.section_elapsed -= duration;
            }
            path.sample(
                self.segment_index,
                self.section_elapsed / duration,
                &mut self.reference,
            );
            return Ok(());
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

    fn settled(&mut self, measured: &[f64], stalled_short: bool) -> anyhow::Result<()> {
        ensure!(
            self.stage == Stage::Settling,
            "Only a settling tracker can settle"
        );
        let segment = &self.plan.segments[self.segment_index];
        self.checkpoints.push(Checkpoint {
            label: segment.label.clone(),
            measured_joints: measured.to_vec(),
            destination_position: segment.destination_position,
            destination_quaternion: segment.destination_quaternion,
        });
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
}

pub struct MotionSupervisor {
    tracker: CornerPathTracker,
    limits: MotionLimits,
    settled_seconds: f64,
    settling_seconds: f64,
}

impl MotionSupervisor {
    pub fn new(
        plan: CornerPlan,
        limits: &MotionLimits,
        joint_count: usize,
    ) -> anyhow::Result<Self> {
        Self::from_validated(ValidatedPlan::new(plan, joint_count)?, limits)
    }

    pub(crate) fn from_validated(
        plan: ValidatedPlan,
        limits: &MotionLimits,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            tracker: CornerPathTracker::new(plan, limits.clone())?,
            limits: limits.clone(),
            settled_seconds: 0.0,
            settling_seconds: 0.0,
        })
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
        self.tracker.end
    }

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
    pub(crate) fn peak_speed(&self) -> f64 {
        self.tracker.plan.peak_speed
    }

    #[must_use]
    pub fn reference(&self) -> &Reference {
        &self.tracker.reference
    }

    #[must_use]
    pub(crate) fn final_pose(&self) -> &[f64] {
        self.tracker.plan.final_corner()
    }

    pub(crate) fn final_gripper_angle(&self) -> Option<hebbian_contracts::JawAngle> {
        self.tracker.plan.final_segment().final_gripper_angle
    }

    pub(crate) fn waypoint_gripper(
        &self,
        checkpoint: usize,
    ) -> Option<hebbian_contracts::JawAngle> {
        self.tracker
            .plan
            .segments
            .get(checkpoint)?
            .pass_through?
            .gripper_angle
    }

    pub(crate) fn new_checkpoints(&self, already_taken: usize) -> &[Checkpoint] {
        &self.tracker.checkpoints[already_taken.min(self.tracker.checkpoints.len())..]
    }

    fn restart_stage(&mut self) {
        self.settled_seconds = 0.0;
        self.settling_seconds = 0.0;
    }

    fn internal(error: &anyhow::Error) -> Trip {
        Trip::Trajectory(format!("{error:#}"))
    }

    // SAFETY: Tracking remains the control loop’s per-cycle check regardless of which trajectory is running.
    pub fn step(&mut self, measured: &[f64], interval: f64) -> Result<Reference, Trip> {
        self.advance(measured, interval)?;
        Ok(self.tracker.reference.clone())
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
        if self.tracker.stage == Stage::Settling {
            let endpoint_error = max_joint_deviation(measured, &self.tracker.reference.positions);
            self.settled_seconds = if endpoint_error <= self.limits.settled_error_radians {
                self.settled_seconds + interval
            } else {
                0.0
            };
            // WHY: Briefly entering tolerance at the deadline cannot extend settling.
            self.settling_seconds += interval;
            let settled = self.settled_seconds >= self.limits.settle_dwell_seconds;
            if settled || self.settling_seconds >= self.limits.settle_timeout_seconds {
                self.tracker
                    .settled(measured, !settled)
                    .map_err(|e| Self::internal(&e))?;
                self.restart_stage();
            }
        }
        if self.tracker.stage == Stage::Moving {
            let previous_corner = self.tracker.corner_index;
            self.tracker
                .advance(measured, interval, speed_fraction)
                .map_err(|e| Self::internal(&e))?;
            if self.tracker.stage != Stage::Moving || self.tracker.corner_index != previous_corner {
                self.restart_stage();
            }
        }
        Ok(())
    }
}

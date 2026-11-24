use anyhow::{Context, Result, ensure};
use hebbian_contracts::{CornerPlan, MotionLimits};
use rsruckig::prelude::{
    DataArrayOrVec, InputParameter, Profile, Ruckig, RuckigResult, ThrowErrorHandler, Trajectory,
};
use rsruckig::util::integrate;

use crate::corner_path::{
    CONTINUITY_RADIANS, INITIAL_PERIOD_SECONDS, plan_initial_corner, validate_plan,
};
use crate::reference_filter::Reference;

pub(crate) struct ContinuousPath {
    trajectory: Trajectory<0>,
    positions: DataArrayOrVec<f64, 0>,
    velocities: DataArrayOrVec<f64, 0>,
    accelerations: DataArrayOrVec<f64, 0>,
}

impl ContinuousPath {
    pub(crate) fn new(
        plan: &CornerPlan,
        limits: &MotionLimits,
        maximum_step_seconds: f64,
    ) -> Result<Self> {
        ensure!(
            plan.is_continuous() && plan.segments.len() >= 2,
            "A continuous move needs one boundary per intermediate waypoint"
        );
        let start = plan_initial_corner(plan)?;
        let joint_count = start.len();
        validate_plan(plan, joint_count)?;
        let (goal, intermediate) = plan
            .segments
            .split_last()
            .context("An empty continuous path")?;
        let mut input = InputParameter::<0>::new(Some(joint_count));
        input.current_position.copy_from_slice(start);
        input.target_position.copy_from_slice(&goal.corners[1]);
        input.intermediate_positions = intermediate
            .iter()
            .map(|segment| DataArrayOrVec::Heap(segment.corners[1].clone()))
            .collect();
        input.max_velocity.fill(plan.peak_speed);
        input.max_acceleration.fill(limits.acceleration_limit);
        input.max_jerk.fill(limits.jerk_limit);
        if maximum_step_seconds > 0.0 {
            // SAFETY: Leave a nanosecond for rounding cumulative section times.
            // Every section must last longer than a capped motor step.
            let minimum_section_seconds = maximum_step_seconds + 1e-9;
            input.per_section_minimum_duration =
                Some(vec![minimum_section_seconds; plan.segments.len()]);
        }

        let mut generator =
            Ruckig::<0, ThrowErrorHandler>::new(Some(joint_count), INITIAL_PERIOD_SECONDS);
        // WHY: The controller regenerates the station's checked plan during startup of the
        // move. Use Ruckig's initial solution to keep calculation off the refinement search.
        generator.waypoint_refinement_sweeps = 0;
        let mut trajectory = Trajectory::new(Some(joint_count));
        let result = generator.calculate(&input, &mut trajectory)?;
        ensure!(
            matches!(result, RuckigResult::Working | RuckigResult::Finished),
            "Ruckig waypoint trajectory failed: {result:?}"
        );

        let section_ends = trajectory.get_intermediate_durations();
        ensure!(
            section_ends.len() == plan.segments.len(),
            "Ruckig did not produce a section for every waypoint and goal"
        );
        let section_starts = std::iter::once(0.0).chain(section_ends.iter().copied());
        for (index, (&end, start)) in section_ends.iter().zip(section_starts).enumerate() {
            ensure!(
                end.is_finite() && end - start > maximum_step_seconds,
                "Continuous section {index} is too short for the control rate"
            );
        }
        for (index, end) in section_ends[..intermediate.len()]
            .iter()
            .copied()
            .enumerate()
        {
            ensure!(
                trajectory
                    .state_at(end)
                    .velocity
                    .iter()
                    .any(|velocity| velocity.abs() > 1e-9),
                "Waypoint {index} requires a stop or reversal; use a separate step"
            );
        }

        Ok(Self {
            trajectory,
            positions: DataArrayOrVec::new(Some(joint_count), 0.0),
            velocities: DataArrayOrVec::new(Some(joint_count), 0.0),
            accelerations: DataArrayOrVec::new(Some(joint_count), 0.0),
        })
    }

    pub(crate) fn section_bounds(&self, index: usize) -> (f64, f64) {
        let ends = self.trajectory.get_intermediate_durations();
        let start = index.checked_sub(1).map_or(0.0, |previous| ends[previous]);
        (start, ends[index])
    }

    pub(crate) fn section_joint_travel_bounds(&self, index: usize) -> Vec<f64> {
        let (start, end) = self.section_bounds(index);
        let duration = end - start;
        self.trajectory.get_profiles()[index]
            .iter()
            .map(|profile| profile_peak_speed(profile, duration) * duration)
            .collect()
    }

    pub(crate) fn check_joint_bounds(
        &self,
        start: &[f64],
        lower: &[f64],
        upper: &[f64],
    ) -> Result<()> {
        for (section, profiles) in self.trajectory.get_profiles().iter().enumerate() {
            for (joint, profile) in profiles.iter().enumerate() {
                let (mut minimum, mut maximum) = (lower[joint], upper[joint]);
                // SAFETY: An already out-of-range joint can only recover inward from its
                // checked initial position. Later sections use the calibrated limits.
                if section == 0 {
                    minimum = minimum.min(start[joint]);
                    maximum = maximum.max(start[joint]);
                }
                let extrema = profile.get_position_extrema();
                ensure!(
                    extrema.min >= minimum - CONTINUITY_RADIANS
                        && extrema.max <= maximum + CONTINUITY_RADIANS,
                    "Continuous section {section}: joint {joint} goes outside the arm's limits"
                );
            }
        }
        Ok(())
    }

    pub(crate) fn sample(&mut self, seconds: f64, reference: &mut Reference) {
        self.trajectory.at_time(
            seconds,
            &mut Some(&mut self.positions),
            &mut Some(&mut self.velocities),
            &mut Some(&mut self.accelerations),
            &mut None,
            &mut None,
        );
        reference.positions.copy_from_slice(&self.positions);
        reference.velocities.copy_from_slice(&self.velocities);
        reference.accelerations.copy_from_slice(&self.accelerations);
    }
}

fn profile_peak_speed(profile: &Profile, duration: f64) -> f64 {
    let mut peak: f64 = 0.0;
    for (index, &phase_duration) in profile.brake.t.iter().enumerate() {
        peak = peak.max(phase_peak_speed(
            phase_duration,
            profile.brake.v[index],
            profile.brake.a[index],
            profile.brake.j[index],
        ));
    }
    for (index, &phase_duration) in profile.t.iter().enumerate() {
        peak = peak.max(phase_peak_speed(
            phase_duration,
            profile.v[index],
            profile.a[index],
            profile.j[index],
        ));
    }
    // SAFETY: Ruckig extrapolates the final state on any remaining section time.
    let tail_duration = (duration - profile.brake.duration - profile.t_sum[6]).max(0.0);
    peak.max(phase_peak_speed(
        tail_duration,
        profile.v[7],
        profile.a[7],
        0.0,
    ))
}

fn phase_peak_speed(duration: f64, velocity: f64, acceleration: f64, jerk: f64) -> f64 {
    let speed_at = |time: f64| {
        let (_, velocity_at_time, _) = integrate(time, 0.0, velocity, acceleration, jerk);
        velocity_at_time.abs()
    };
    let mut peak = velocity.abs().max(speed_at(duration));
    // SAFETY: With constant jerk, a velocity extremum is at an endpoint or
    // where acceleration crosses zero.
    if jerk != 0.0 {
        let stationary_time = -acceleration / jerk;
        if (0.0..duration).contains(&stationary_time) {
            peak = peak.max(speed_at(stationary_time));
        }
    }
    peak
}

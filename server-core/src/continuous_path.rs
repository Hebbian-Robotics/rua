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
    pub(crate) peak_joint_speed: f64,
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
        input.max_acceleration.fill(limits.acceleration_limit);
        input.max_jerk.fill(limits.jerk_limit);
        if maximum_step_seconds > 0.0
            || plan
                .segments
                .iter()
                .any(|segment| segment.minimum_duration_seconds.is_some())
        {
            // SAFETY: Every section must last longer than a capped motor step.
            // Leave a nanosecond for rounding cumulative section times.
            let control_cycle_floor = maximum_step_seconds + 1e-9;
            input.per_section_minimum_duration = Some(
                plan.segments
                    .iter()
                    .map(|segment| {
                        segment
                            .minimum_duration_seconds
                            .unwrap_or(0.0)
                            .max(control_cycle_floor)
                    })
                    .collect(),
            );
        }

        let trajectory = calculate_waypoints(plan, limits, input)?;

        let section_ends = trajectory.get_intermediate_durations();
        ensure!(
            section_ends.len() == plan.segments.len(),
            "Ruckig did not produce a section for every waypoint and goal"
        );
        let section_starts = std::iter::once(0.0).chain(section_ends.iter().copied());
        let mut peak_joint_speed: f64 = 0.0;
        for (index, (&end, start)) in section_ends.iter().zip(section_starts).enumerate() {
            ensure!(
                end.is_finite() && end - start > maximum_step_seconds,
                "Continuous section {index} is too short for the control rate"
            );
            for profile in trajectory.get_profiles()[index].iter() {
                peak_joint_speed = peak_joint_speed.max(profile_peak_speed(profile, end - start));
            }
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

        ensure!(
            peak_joint_speed.is_finite() && peak_joint_speed > 0.0,
            "Invalid continuous path speed"
        );
        Ok(Self {
            trajectory,
            peak_joint_speed,
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

fn calculate_waypoints(
    plan: &CornerPlan,
    limits: &MotionLimits,
    mut input: InputParameter<0>,
) -> Result<Trajectory<0>> {
    let joint_count = input.degrees_of_freedom;
    input.max_velocity.fill(plan.peak_speed);
    let (minimum, maximum) = waypoint_velocity_limits(plan, limits);
    input.per_section_min_velocity = Some(minimum);
    input.per_section_max_velocity = Some(maximum);

    // WHY: Use the initial solution to bound solve time when the controller regenerates the checked plan.
    let mut generator =
        Ruckig::<0, ThrowErrorHandler>::new(Some(joint_count), INITIAL_PERIOD_SECONDS);
    generator.waypoint_refinement_sweeps = 0;
    let mut trajectory = Trajectory::new(Some(joint_count));
    let result = generator.calculate(&input, &mut trajectory)?;
    ensure!(
        matches!(result, RuckigResult::Working | RuckigResult::Finished),
        "Ruckig waypoint trajectory failed: {result:?}"
    );
    Ok(trajectory)
}

fn waypoint_velocity_limits(
    plan: &CornerPlan,
    limits: &MotionLimits,
) -> (Vec<DataArrayOrVec<f64, 0>>, Vec<DataArrayOrVec<f64, 0>>) {
    // WHY: A joint with a short
    // displacement should not accelerate to the same speed as the leading joint.
    let section_deltas: Vec<Vec<f64>> = plan
        .segments
        .iter()
        .map(|segment| {
            segment.corners[0]
                .iter()
                .zip(&segment.corners[1])
                .map(|(start, end)| end - start)
                .collect()
        })
        .collect();
    let largest_displacement = section_deltas
        .iter()
        .flatten()
        .map(|delta| delta.abs())
        .fold(0.0_f64, f64::max);
    // WHY: A shared time scale gives neighboring sections compatible crossing
    // speeds. Leave headroom for Ruckig to synchronize joints and brake at turns.
    let spacing = largest_displacement.max(f64::MIN_POSITIVE);
    let acceleration_speed = (limits.acceleration_limit * spacing).sqrt();
    let jerk_speed = (limits.jerk_limit * spacing * spacing).cbrt();
    let ceiling = plan
        .peak_speed
        .min(0.5 * acceleration_speed.min(jerk_speed));
    let directional_limits = |direction: f64| {
        section_deltas
            .iter()
            .map(|deltas| {
                DataArrayOrVec::Heap(
                    deltas
                        .iter()
                        .map(|delta| direction * ceiling * (direction * delta).max(0.0) / spacing)
                        .collect(),
                )
            })
            .collect()
    };
    (directional_limits(-1.0), directional_limits(1.0))
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

#[cfg(test)]
mod tests {
    use hebbian_contracts::{PassThrough, Segment};

    use super::*;
    use crate::config::{ControllerConfig, TEST_CONFIG};

    #[test]
    fn circular_waypoints_do_not_reverse_inside_a_section() {
        let knots: Vec<Vec<f64>> =
            serde_json::from_str(include_str!("../tests/support/so101_circle_waypoints.json"))
                .unwrap();
        let segments = knots
            .windows(2)
            .enumerate()
            .map(|(index, pair)| Segment {
                label: format!("waypoint_{index}"),
                corners: pair.to_vec(),
                pass_through: (index + 2 < knots.len()).then_some(PassThrough {
                    gripper_angle: None,
                }),
                minimum_duration_seconds: None,
                final_gripper_angle: None,
                destination_position: None,
                destination_quaternion: None,
            })
            .collect();
        let plan = CornerPlan {
            segments,
            peak_speed: 2.5,
            trajectory_limits: None,
        };
        let mut limits = ControllerConfig::parse(TEST_CONFIG)
            .unwrap()
            .into_input()
            .motion;
        limits.acceleration_limit = 5.0;
        limits.jerk_limit = 80.0;
        let path = ContinuousPath::new(&plan, &limits, 0.04).unwrap();
        assert!((0.0..0.30).contains(&path.peak_joint_speed));
        for (section, pair) in knots.windows(2).enumerate() {
            let (start, end) = path.section_bounds(section);
            for sample in 0..=100 {
                let state = path
                    .trajectory
                    .state_at(start + (end - start) * f64::from(sample) / 100.0);
                for (joint, &velocity) in state.velocity.iter().enumerate() {
                    let delta = pair[1][joint] - pair[0][joint];
                    assert!(
                        velocity.abs() < 1e-8 || (delta.abs() > 1e-12 && velocity * delta > 0.0),
                        "Section {section}, joint {joint}: velocity {velocity} for displacement {delta}"
                    );
                }
            }
        }
    }
}

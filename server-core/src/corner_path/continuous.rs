use anyhow::{Context, Result, ensure};
use hebbian_contracts::{CornerPlan, MotionLimits};
use rsruckig::prelude::{
    ControlInterface, DataArrayOrVec, InputParameter, OutputParameter, Ruckig, RuckigResult,
    ThrowErrorHandler, Trajectory,
};

use super::{CONTINUITY_RADIANS, INITIAL_PERIOD_SECONDS, plan_initial_corner, validate_plan};
use crate::reference_filter::Reference;

pub(crate) struct ContinuousPath {
    trajectory: Trajectory<0>,
    pub(crate) peak_joint_speed: f64,
    positions: DataArrayOrVec<f64, 0>,
    velocities: DataArrayOrVec<f64, 0>,
    accelerations: DataArrayOrVec<f64, 0>,
    clock_generator: Ruckig<1, ThrowErrorHandler>,
    clock_input: InputParameter<1>,
    clock_output: OutputParameter<1>,
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
        // WHY: Reserve half the acceleration and jerk for smoothly changing the path clock.
        input.max_acceleration.fill(limits.acceleration_limit * 0.5);
        input.max_jerk.fill(limits.jerk_limit * 0.5);
        if maximum_step_seconds > 0.0
            || plan
                .segments
                .iter()
                .any(|segment| segment.minimum_duration_seconds.is_some())
        {
            // CONSTRAINT: Every section must last longer than a capped motor step.
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
            for joint in 0..joint_count {
                peak_joint_speed =
                    peak_joint_speed.max(section_peak_speed(&trajectory, index, joint)?);
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
        let mut clock_input = InputParameter::<1>::new(None);
        clock_input.control_interface = ControlInterface::Velocity;
        clock_input.current_velocity[0] = 1.0;
        clock_input.target_velocity[0] = 1.0;
        // WHY: The chain rule bounds q'' = q_tt * clock_velocity² + q_t * clock_acceleration,
        // q''' = q_ttt * clock_velocity³ + 3*q_tt*clock_velocity*clock_acceleration
        //          + q_t*clock_jerk. Allocate the remaining limit to those terms.
        clock_input.max_acceleration[0] = (0.5 * limits.acceleration_limit / peak_joint_speed)
            .min(limits.jerk_limit / (6.0 * limits.acceleration_limit));
        clock_input.max_jerk[0] = limits.jerk_limit / (4.0 * peak_joint_speed);
        Ok(Self {
            trajectory,
            peak_joint_speed,
            positions: DataArrayOrVec::new(Some(joint_count), 0.0),
            velocities: DataArrayOrVec::new(Some(joint_count), 0.0),
            accelerations: DataArrayOrVec::new(Some(joint_count), 0.0),
            clock_generator: Ruckig::new(None, INITIAL_PERIOD_SECONDS),
            clock_input,
            clock_output: OutputParameter::new(None),
        })
    }

    pub(crate) fn section_bounds(&self, index: usize) -> (f64, f64) {
        let ends = self.trajectory.get_intermediate_durations();
        let start = index.checked_sub(1).map_or(0.0, |previous| ends[previous]);
        (start, ends[index])
    }

    pub(crate) fn section_joint_travel_bounds(&self, index: usize) -> Result<Vec<f64>> {
        let (start, end) = self.section_bounds(index);
        let duration = end - start;
        (0..self.positions.len())
            .map(|joint| {
                section_peak_speed(&self.trajectory, index, joint).map(|speed| speed * duration)
            })
            .collect()
    }

    pub(crate) fn check_joint_bounds(
        &self,
        start: &[f64],
        lower: &[f64],
        upper: &[f64],
    ) -> Result<()> {
        for (section, profiles) in self.trajectory.get_profiles().iter().enumerate() {
            let (section_start, section_end) = self.section_bounds(section);
            let section_duration = section_end - section_start;
            for (joint, profile) in profiles.iter().enumerate() {
                let (mut minimum, mut maximum) = (lower[joint], upper[joint]);
                // ROBOT-SAFETY: An already out-of-range joint can only recover inward from its
                // checked initial position. Later sections use the calibrated limits.
                if section == 0 {
                    minimum = minimum.min(start[joint]);
                    maximum = maximum.max(start[joint]);
                }
                let extrema = profile.get_position_extrema_until(section_duration);
                ensure!(
                    extrema.min >= minimum - CONTINUITY_RADIANS
                        && extrema.max <= maximum + CONTINUITY_RADIANS,
                    "Continuous section {section}: joint {joint} goes outside the arm's limits"
                );
            }
        }
        Ok(())
    }

    pub(crate) fn advance_clock(&mut self, interval: f64, speed_fraction: f64) -> Result<f64> {
        ensure!(
            interval.is_finite()
                && interval > 0.0
                && speed_fraction.is_finite()
                && (super::MIN_TRACKING_SPEED_FRACTION..=1.0).contains(&speed_fraction),
            "Invalid continuous path clock request"
        );
        self.clock_generator.delta_time = interval;
        self.clock_input.target_velocity[0] = speed_fraction;
        let result = self
            .clock_generator
            .update(&self.clock_input, &mut self.clock_output)?;
        ensure!(
            matches!(result, RuckigResult::Working | RuckigResult::Finished),
            "Continuous path clock failed: {result:?}"
        );
        ensure!(
            (0.0..=1.000_000_001).contains(&self.clock_output.new_velocity[0]),
            "Continuous path clock exceeded its speed bounds"
        );
        self.clock_output.pass_to_input(&mut self.clock_input);
        Ok(self.clock_output.new_position[0])
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
        let clock_velocity = self.clock_output.new_velocity[0];
        let clock_acceleration = self.clock_output.new_acceleration[0];
        for joint in 0..self.positions.len() {
            reference.velocities[joint] = self.velocities[joint] * clock_velocity;
            reference.accelerations[joint] = self.accelerations[joint] * clock_velocity.powi(2)
                + self.velocities[joint] * clock_acceleration;
        }
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

fn section_peak_speed(trajectory: &Trajectory<0>, section: usize, joint: usize) -> Result<f64> {
    let extrema = trajectory
        .get_velocity_extrema(section, joint)
        .with_context(|| {
            format!("Missing velocity extrema for section {section}, joint {joint}")
        })?;
    Ok(extrema.min.abs().max(extrema.max.abs()))
}

#[cfg(test)]
mod tests {
    use hebbian_contracts::{PassThrough, Segment};

    use super::*;
    use crate::config::{ControllerConfig, TEST_CONFIG};

    #[test]
    fn joint_bounds_include_motion_after_an_axis_finishes() {
        use rsruckig::prelude::Synchronization;

        let mut input = InputParameter::<0>::new(Some(2));
        input.synchronization = Synchronization::None;
        input.target_position[0] = 0.5;
        input.target_position[1] = 10.0;
        input.target_velocity[0] = 0.5;
        input.max_velocity.fill(1.0);
        input.max_acceleration.fill(1.0);
        input.max_jerk.fill(1.0);
        let mut trajectory = Trajectory::<0>::new(Some(2));
        let result = Ruckig::<0, ThrowErrorHandler>::new(Some(2), 0.01)
            .calculate(&input, &mut trajectory)
            .unwrap();
        assert!(matches!(
            result,
            RuckigResult::Working | RuckigResult::Finished
        ));
        let path = ContinuousPath {
            trajectory,
            peak_joint_speed: 1.0,
            positions: DataArrayOrVec::new(Some(2), 0.0),
            velocities: DataArrayOrVec::new(Some(2), 0.0),
            accelerations: DataArrayOrVec::new(Some(2), 0.0),
            clock_generator: Ruckig::new(None, INITIAL_PERIOD_SECONDS),
            clock_input: InputParameter::new(None),
            clock_output: OutputParameter::new(None),
        };
        assert!(
            path.check_joint_bounds(&[0.0, 0.0], &[-1.0, -1.0], &[6.0, 11.0])
                .is_ok()
        );
        assert!(
            path.check_joint_bounds(&[0.0, 0.0], &[-1.0, -1.0], &[1.5, 11.0])
                .is_err()
        );
    }

    #[test]
    fn circular_waypoints_do_not_reverse_inside_a_section() {
        let knots: Vec<Vec<f64>> = serde_json::from_str(include_str!(
            "../../tests/support/so101_circle_waypoints.json"
        ))
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
            let travel_bounds = path.section_joint_travel_bounds(section).unwrap();
            for sample in 0..=100 {
                let state = path
                    .trajectory
                    .state_at(start + (end - start) * f64::from(sample) / 100.0);
                for (joint, &velocity) in state.velocity.iter().enumerate() {
                    let delta = pair[1][joint] - pair[0][joint];
                    assert!(velocity.abs() <= path.peak_joint_speed + 1e-9);
                    assert!(velocity.abs() * (end - start) <= travel_bounds[joint] + 1e-9);
                    assert!(
                        velocity.abs() < 1e-8 || (delta.abs() > 1e-12 && velocity * delta > 0.0),
                        "Section {section}, joint {joint}: velocity {velocity} for displacement {delta}"
                    );
                }
            }
        }
    }
}

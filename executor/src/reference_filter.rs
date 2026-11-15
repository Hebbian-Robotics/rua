//! The online Ruckig smoother between a goal and the motors. Each cycle it moves
//! from its last output toward the newest reference within the profile's one
//! set of velocity (`max_joint_speed`), acceleration and jerk limits, the same
//! ones planned motions use, independently per joint. Planned motions and policy
//! output bypass it but keep its state current, so a stop brakes from the
//! commanded velocity.

use rsruckig::prelude::{
    ControlInterface, InputParameter, OutputParameter, Ruckig, RuckigResult, Synchronization,
    ThrowErrorHandler, Trajectory,
};

use crate::config::ExecutorConfig;
use crate::joint_limits::clamp_within_recovery;
use crate::limits::{Trip, is_finite_joint_vector};

/// Ruckig refuses target states beyond its own caps; aim just inside them.
const INSIDE_CAP: f64 = 0.999;

/// Where the arm should go, and how fast it may approach.
#[derive(Debug, Clone)]
pub struct Reference {
    pub positions: Vec<f64>,
    pub velocities: Vec<f64>,
    pub accelerations: Vec<f64>,
    pub speed_limit: Option<f64>,
}

impl Reference {
    #[must_use]
    pub(crate) fn at_rest(positions: &[f64], speed_limit: Option<f64>) -> Self {
        Self {
            positions: positions.to_vec(),
            velocities: vec![0.0; positions.len()],
            accelerations: vec![0.0; positions.len()],
            speed_limit,
        }
    }

    /// Copy a reference while retaining the destination's per-joint buffers.
    pub(crate) fn copy_from(&mut self, source: &Self) {
        self.positions.clone_from(&source.positions);
        self.velocities.clone_from(&source.velocities);
        self.accelerations.clone_from(&source.accelerations);
        self.speed_limit = source.speed_limit;
    }

    /// Become `at_rest` without a new allocation once the buffers have grown.
    pub(crate) fn set_at_rest(&mut self, positions: &[f64], speed_limit: Option<f64>) {
        self.positions.clear();
        self.positions.extend_from_slice(positions);
        self.velocities.clear();
        self.velocities.resize(positions.len(), 0.0);
        self.accelerations.clear();
        self.accelerations.resize(positions.len(), 0.0);
        self.speed_limit = speed_limit;
    }
}

/// Numerical noise allowed when evaluating Ruckig at a joint boundary.
const ROUNDING_TOLERANCE_RADIANS: f64 = 1e-10;

pub(crate) struct ReferenceFilter {
    joint_count: usize,
    lower_limits: Vec<f64>,
    upper_limits: Vec<f64>,
    recovery_lower: Vec<f64>,
    recovery_upper: Vec<f64>,
    recovering_inward: Vec<i8>,
    velocity_limit: f64,
    acceleration_limit: f64,
    jerk_limit: f64,
    interval: f64,
    generator: Ruckig<0, ThrowErrorHandler>,
    input: InputParameter<0>,
    output: OutputParameter<0>,
    positions: Vec<f64>,
    velocities: Vec<f64>,
    accelerations: Vec<f64>,
}

impl ReferenceFilter {
    /// Start at rest at `initial`.
    ///
    /// # Errors
    /// The start is not finite, or is too far outside the joint limits to recover.
    pub fn new(config: &ExecutorConfig, initial: &[f64]) -> anyhow::Result<Self> {
        let n = config.joint_count;
        if !is_finite_joint_vector(initial, n) {
            anyhow::bail!("Invalid filter start");
        }
        if clamp_within_recovery(
            initial,
            &config.lower_limits,
            &config.upper_limits,
            config.joint_limit_recovery_tolerance_radians,
        )
        .is_err()
        {
            anyhow::bail!("Filter start exceeds joint-limit recovery tolerance");
        }
        let velocity_limit = config.max_joint_speed;
        let acceleration_limit = config.motion.acceleration_limit;
        let jerk_limit = config.motion.jerk_limit;
        let mut input = InputParameter::<0>::new(Some(n));
        input
            .max_velocity
            .iter_mut()
            .for_each(|v| *v = velocity_limit);
        input
            .max_acceleration
            .iter_mut()
            .for_each(|a| *a = acceleration_limit);
        input.max_jerk.iter_mut().for_each(|j| *j = jerk_limit);
        // Each joint independently: the filter bounds motion, it does not shape paths.
        input.synchronization = Synchronization::None;
        Ok(Self {
            joint_count: n,
            recovering_inward: vec![0; n],
            recovery_lower: initial
                .iter()
                .zip(&config.lower_limits)
                .map(|(start, lower)| start.min(*lower))
                .collect(),
            recovery_upper: initial
                .iter()
                .zip(&config.upper_limits)
                .map(|(start, upper)| start.max(*upper))
                .collect(),
            lower_limits: config.lower_limits.clone(),
            upper_limits: config.upper_limits.clone(),
            velocity_limit,
            acceleration_limit,
            jerk_limit,
            interval: 1.0 / config.frequency_hertz,
            generator: Ruckig::<0, ThrowErrorHandler>::new(Some(n), 1.0 / config.frequency_hertz),
            input,
            output: OutputParameter::<0>::new(Some(n)),
            positions: initial.to_vec(),
            velocities: vec![0.0; n],
            accelerations: vec![0.0; n],
        })
    }

    #[must_use]
    pub fn positions(&self) -> &[f64] {
        &self.positions
    }

    /// Start at rest at these positions.
    pub fn reset(&mut self, positions: &[f64]) {
        self.follow(&Reference::at_rest(positions, None));
    }

    /// A planned motion or policy commanded this reference directly: carry its
    /// state, so the smoother continues from it.
    pub fn follow(&mut self, reference: &Reference) {
        self.positions.clone_from(&reference.positions);
        self.velocities.clone_from(&reference.velocities);
        self.accelerations.clone_from(&reference.accelerations);
        self.recovering_inward.fill(0);
        // A start just outside a limit (an arm resting against its stop) is the
        // new edge of what the filter may output until it is back inside.
        for joint in 0..self.joint_count {
            self.recovery_lower[joint] = self.lower_limits[joint].min(self.positions[joint]);
            self.recovery_upper[joint] = self.upper_limits[joint].max(self.positions[joint]);
        }
    }

    /// Where the arm comes to rest if it brakes now within the limits.
    #[must_use]
    pub(crate) fn stopping_position(&self) -> Vec<f64> {
        let n = self.joint_count;
        let mut generator = Ruckig::<0, ThrowErrorHandler>::new(Some(n), self.interval);
        let mut input = InputParameter::<0>::new(Some(n));
        input.control_interface = ControlInterface::Velocity;
        input.synchronization = Synchronization::None;
        for joint in 0..n {
            input.max_velocity[joint] = self.velocity_limit;
            input.max_acceleration[joint] = self.acceleration_limit;
            input.max_jerk[joint] = self.jerk_limit;
            input.current_position[joint] = self.positions[joint];
            input.current_velocity[joint] = self.velocities[joint];
            input.current_acceleration[joint] = self.accelerations[joint];
        }
        let mut trajectory = Trajectory::<0>::new(Some(n));
        if generator.calculate(&input, &mut trajectory).is_err() {
            return self.positions.clone();
        }
        let mut stopped = input.current_position.clone();
        trajectory.at_time(
            trajectory.get_duration(),
            &mut Some(&mut stopped),
            &mut None,
            &mut None,
            &mut None,
            &mut None,
        );
        (0..n)
            .map(|joint| {
                stopped[joint].clamp(self.recovery_lower[joint], self.recovery_upper[joint])
            })
            .collect()
    }

    fn recovery_target(&mut self, joint: usize, wanted: f64) -> f64 {
        // When an inward path starts outside a calibrated limit, aim the
        // filter at the limit until it has recovered. Chasing successive
        // outside waypoints can reverse velocity and move outward again.
        if self.recovering_inward[joint] == 0 {
            if self.positions[joint] > self.upper_limits[joint]
                && wanted < self.positions[joint] - 1e-8
            {
                self.recovering_inward[joint] = 1;
            } else if self.positions[joint] < self.lower_limits[joint]
                && wanted > self.positions[joint] + 1e-8
            {
                self.recovering_inward[joint] = -1;
            }
        } else if self.positions[joint] >= self.lower_limits[joint]
            && self.positions[joint] <= self.upper_limits[joint]
            && wanted >= self.lower_limits[joint]
            && wanted <= self.upper_limits[joint]
            && self.velocities[joint].abs() < 1e-6
            && self.accelerations[joint].abs() < 1e-5
        {
            self.recovering_inward[joint] = 0;
        }
        match self.recovering_inward[joint] {
            1 => {
                self.upper_limits[joint]
                    - 0.002_f64.min((self.upper_limits[joint] - self.lower_limits[joint]) * 0.1)
            }
            -1 => {
                self.lower_limits[joint]
                    + 0.002_f64.min((self.upper_limits[joint] - self.lower_limits[joint]) * 0.1)
            }
            _ => wanted.clamp(self.recovery_lower[joint], self.recovery_upper[joint]),
        }
    }

    /// The next command, `interval` seconds after the last, written to `command`.
    ///
    /// # Errors
    /// A `Trip` when the reference is malformed or the filter cannot follow it.
    pub(crate) fn apply(
        &mut self,
        reference: &Reference,
        interval: f64,
        command: &mut Vec<f64>,
    ) -> Result<(), Trip> {
        let n = self.joint_count;
        if !(is_finite_joint_vector(&reference.positions, n)
            && is_finite_joint_vector(&reference.velocities, n)
            && is_finite_joint_vector(&reference.accelerations, n))
        {
            return Err(Trip(
                "Tracking controller produced an invalid reference".into(),
            ));
        }
        let velocity_limit = reference
            .speed_limit
            .map_or(self.velocity_limit, |limit| self.velocity_limit.min(limit));
        for joint in 0..n {
            let wanted = reference.positions[joint];
            let target = self.recovery_target(joint, wanted);
            // `clamp` returns `wanted` itself when it is in range, so exact
            // equality is the right test for "was moved to a limit".
            #[allow(clippy::float_cmp)]
            let at_limit = target != wanted || self.recovering_inward[joint] != 0;
            self.input.max_velocity[joint] = velocity_limit;
            self.input.current_position[joint] = self.positions[joint];
            self.input.current_velocity[joint] = self.velocities[joint];
            self.input.current_acceleration[joint] = self.accelerations[joint];
            self.input.target_position[joint] = target;
            self.input.target_velocity[joint] = if at_limit {
                0.0
            } else {
                reference.velocities[joint]
                    .clamp(-INSIDE_CAP * velocity_limit, INSIDE_CAP * velocity_limit)
            };
            self.input.target_acceleration[joint] = if at_limit {
                0.0
            } else {
                reference.accelerations[joint].clamp(
                    -INSIDE_CAP * self.acceleration_limit,
                    INSIDE_CAP * self.acceleration_limit,
                )
            };
        }
        // Current state comes from the guarded values above, so a clamp,
        // reset or followed motion also invalidates Ruckig's cached prediction.
        self.generator.delta_time = interval;
        match self.generator.update(&self.input, &mut self.output) {
            Ok(RuckigResult::Working | RuckigResult::Finished) => {}
            Ok(other) => return Err(Trip(format!("Reference filter failed: {other:?}"))),
            Err(error) => return Err(Trip(format!("Reference filter failed: {error}"))),
        }
        // Past its end Ruckig extrapolates the target state, which is what a
        // reference that keeps moving at the target velocity would do.
        let positions = &mut self.output.new_position;
        let velocities = &self.output.new_velocity;
        let accelerations = &self.output.new_acceleration;
        // Clamp only picoradian-scale numerical rounding at a boundary.
        for (joint, position) in positions.iter_mut().enumerate() {
            if *position < self.recovery_lower[joint] - ROUNDING_TOLERANCE_RADIANS
                || *position > self.recovery_upper[joint] + ROUNDING_TOLERANCE_RADIANS
            {
                return Err(Trip(format!(
                    "Filtered reference left the joint limits: joint {joint}, position {position:.12}, bounds {:.12}..{:.12}, velocity {:.12}, prev {:.12}, ref {:.12}, target {:.12}, latched {}",
                    self.recovery_lower[joint],
                    self.recovery_upper[joint],
                    velocities[joint],
                    self.positions[joint],
                    reference.positions[joint],
                    self.input.target_position[joint],
                    self.recovering_inward[joint]
                )));
            }
            *position = position.clamp(self.recovery_lower[joint], self.recovery_upper[joint]);
        }
        for joint in 0..n {
            self.recovery_lower[joint] = self.lower_limits[joint].min(positions[joint]);
            self.recovery_upper[joint] = self.upper_limits[joint].max(positions[joint]);
            self.positions[joint] = positions[joint];
            self.velocities[joint] = velocities[joint];
            self.accelerations[joint] = accelerations[joint];
        }
        command.clear();
        command.extend_from_slice(&self.positions);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::TEST_CONFIG;

    fn applied(filter: &mut ReferenceFilter, goal: &Reference) -> Vec<f64> {
        let mut command = Vec::new();
        filter.apply(goal, 0.01, &mut command).unwrap();
        command
    }

    fn filter_at(start: &[f64]) -> ReferenceFilter {
        ReferenceFilter::new(&ExecutorConfig::parse(TEST_CONFIG).unwrap(), start).unwrap()
    }

    #[test]
    fn the_filter_arrives_without_exceeding_its_speed_cap() {
        for intervals in [&[0.01][..], &[0.001, 0.007, 0.019, 0.031, 0.012][..]] {
            let mut filter = filter_at(&[0.0, 0.0]);
            let goal = Reference::at_rest(&[1.0, -0.5], Some(1.0));
            let mut previous = vec![0.0, 0.0];
            let mut command = Vec::new();
            for cycle in 0..400 {
                let interval = intervals[cycle % intervals.len()];
                filter.apply(&goal, interval, &mut command).unwrap();
                for joint in 0..2 {
                    assert!((command[joint] - previous[joint]).abs() / interval <= 1.0 + 1e-6);
                }
                previous.clone_from(&command);
            }
            assert!((previous[0] - 1.0).abs() < 1e-3 && (previous[1] + 0.5).abs() < 1e-3);
        }
    }

    #[test]
    fn a_goal_outside_the_joint_limits_is_clipped_to_them() {
        let mut filter = filter_at(&[0.0, 0.0]);
        let goal = Reference::at_rest(&[5.0, 0.0], None);
        let mut last = vec![0.0, 0.0];
        for _ in 0..600 {
            last = applied(&mut filter, &goal);
        }
        assert!((last[0] - 2.0).abs() < 1e-3);
    }

    #[test]
    fn braking_from_speed_carries_on_to_rest_within_the_limits() {
        let mut filter = filter_at(&[0.0, 0.0]);
        filter.follow(&Reference {
            positions: vec![0.0, 0.0],
            velocities: vec![1.0, 0.0],
            accelerations: vec![0.0, 0.0],
            speed_limit: None,
        });
        let rest = filter.stopping_position();
        assert!(rest[0] > 0.0 && rest[0] < 1.0 && rest[1].abs() < 1e-9);
        let goal = Reference::at_rest(&rest, None);
        let mut previous = 0.0;
        let mut previous_speed = 1.0;
        for _ in 0..300 {
            let command = applied(&mut filter, &goal)[0];
            let speed = (command - previous) / 0.01;
            assert!(speed >= -1e-6 && (speed - previous_speed).abs() / 0.01 <= 20.0 + 1e-6);
            previous = command;
            previous_speed = speed;
        }
        assert!((previous - rest[0]).abs() < 1e-3);
    }

    #[test]
    fn a_non_finite_reference_trips() {
        let mut filter = filter_at(&[0.0, 0.0]);
        let bad = Reference::at_rest(&[f64::NAN, 0.0], None);
        assert!(filter.apply(&bad, 0.01, &mut Vec::new()).is_err());
    }

    #[test]
    fn a_stream_may_start_just_outside_a_limit_and_move_back_in() {
        let mut filter = filter_at(&[0.0, 0.0]);
        // The arm rests 1.5 mrad past the upper limit of 2.0, as one resting on its stop does.
        filter.reset(&[2.0015, 0.0]);
        let goal = Reference::at_rest(&[1.5, 0.0], None);
        let mut last = vec![2.0015, 0.0];
        for _ in 0..300 {
            last = applied(&mut filter, &goal);
            assert!(last[0] <= 2.0015 + 1e-9, "moved further out: {}", last[0]);
        }
        assert!((last[0] - 1.5).abs() < 1e-3);
    }

    #[test]
    fn a_start_far_outside_the_limits_is_refused() {
        let config = ExecutorConfig::parse(TEST_CONFIG).unwrap();
        assert!(ReferenceFilter::new(&config, &[2.5, 0.0]).is_err());
    }
}

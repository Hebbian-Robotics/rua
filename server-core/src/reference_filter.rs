// INVARIANT: Direct planned and policy outputs must keep the smoother’s state current so stops brake from commanded velocity.

use anyhow::Context;
use rsruckig::prelude::{
    ControlInterface, InputParameter, OutputParameter, Ruckig, RuckigResult, Synchronization,
    ThrowErrorHandler, Trajectory,
};

use crate::config::ControllerConfig;
use crate::limits::{Trip, is_finite_joint_vector};

// CONSTRAINT: Ruckig refuses target states beyond its own caps; aim just inside them.
const INSIDE_CAP: f64 = 0.999;

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

    pub(crate) fn copy_from(&mut self, source: &Self) {
        self.positions.clone_from(&source.positions);
        self.velocities.clone_from(&source.velocities);
        self.accelerations.clone_from(&source.accelerations);
        self.speed_limit = source.speed_limit;
    }

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

const ROUNDING_TOLERANCE_RADIANS: f64 = 1e-10;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InwardRecovery {
    Inactive,
    FromAbove,
    FromBelow,
}

enum JointTarget {
    Unconstrained(f64),
    Clamped(f64),
    Recovering(f64),
}

pub(crate) struct ReferenceFilter {
    joint_count: usize,
    lower_limits: Vec<f64>,
    upper_limits: Vec<f64>,
    recovery_lower: Vec<f64>,
    recovery_upper: Vec<f64>,
    recovering_inward: Vec<InwardRecovery>,
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
    pub fn new(config: &ControllerConfig, initial: &[f64]) -> anyhow::Result<Self> {
        let n = config.joint_count;
        config
            .recovery_limits()
            .clamp(initial)
            .context("Invalid filter start")?;
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
        // WHY: Each joint independently: the filter bounds motion, it does not shape paths.
        input.synchronization = Synchronization::None;
        Ok(Self {
            joint_count: n,
            recovering_inward: vec![InwardRecovery::Inactive; n],
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

    pub fn reset(&mut self, positions: &[f64]) {
        self.follow(&Reference::at_rest(positions, None));
    }

    pub fn follow(&mut self, reference: &Reference) {
        self.positions.clone_from(&reference.positions);
        self.velocities.clone_from(&reference.velocities);
        self.accelerations.clone_from(&reference.accelerations);
        self.recovering_inward.fill(InwardRecovery::Inactive);
        // INVARIANT: A start just outside a limit (an arm resting against its stop) is the
        // new edge of what the filter may output until it is back inside.
        for joint in 0..self.joint_count {
            self.recovery_lower[joint] = self.lower_limits[joint].min(self.positions[joint]);
            self.recovery_upper[joint] = self.upper_limits[joint].max(self.positions[joint]);
        }
    }

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

    fn recovery_target(&mut self, joint: usize, wanted: f64) -> JointTarget {
        // WHY: When an inward path starts outside a calibrated limit, aim the
        // filter at the limit until it has recovered. Chasing successive
        // outside waypoints can reverse velocity and move outward again.
        if self.recovering_inward[joint] == InwardRecovery::Inactive {
            if self.positions[joint] > self.upper_limits[joint]
                && wanted < self.positions[joint] - 1e-8
            {
                self.recovering_inward[joint] = InwardRecovery::FromAbove;
            } else if self.positions[joint] < self.lower_limits[joint]
                && wanted > self.positions[joint] + 1e-8
            {
                self.recovering_inward[joint] = InwardRecovery::FromBelow;
            }
        } else if self.positions[joint] >= self.lower_limits[joint]
            && self.positions[joint] <= self.upper_limits[joint]
            && wanted >= self.lower_limits[joint]
            && wanted <= self.upper_limits[joint]
            && self.velocities[joint].abs() < 1e-6
            && self.accelerations[joint].abs() < 1e-5
        {
            self.recovering_inward[joint] = InwardRecovery::Inactive;
        }
        match self.recovering_inward[joint] {
            InwardRecovery::FromAbove => JointTarget::Recovering(
                self.upper_limits[joint]
                    - 0.002_f64.min((self.upper_limits[joint] - self.lower_limits[joint]) * 0.1),
            ),
            InwardRecovery::FromBelow => JointTarget::Recovering(
                self.lower_limits[joint]
                    + 0.002_f64.min((self.upper_limits[joint] - self.lower_limits[joint]) * 0.1),
            ),
            InwardRecovery::Inactive => {
                let lower = self.recovery_lower[joint];
                let upper = self.recovery_upper[joint];
                if (lower..=upper).contains(&wanted) {
                    JointTarget::Unconstrained(wanted)
                } else {
                    JointTarget::Clamped(wanted.clamp(lower, upper))
                }
            }
        }
    }

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
            return Err(Trip::ReferenceFilter(
                "Tracking controller produced an invalid reference".into(),
            ));
        }
        let velocity_limit = reference
            .speed_limit
            .map_or(self.velocity_limit, |limit| self.velocity_limit.min(limit));
        for joint in 0..n {
            let (position, velocity, acceleration) =
                match self.recovery_target(joint, reference.positions[joint]) {
                    JointTarget::Unconstrained(position) => (
                        position,
                        reference.velocities[joint]
                            .clamp(-INSIDE_CAP * velocity_limit, INSIDE_CAP * velocity_limit),
                        reference.accelerations[joint].clamp(
                            -INSIDE_CAP * self.acceleration_limit,
                            INSIDE_CAP * self.acceleration_limit,
                        ),
                    ),
                    JointTarget::Clamped(position) | JointTarget::Recovering(position) => {
                        (position, 0.0, 0.0)
                    }
                };
            self.input.max_velocity[joint] = velocity_limit;
            self.input.current_position[joint] = self.positions[joint];
            self.input.current_velocity[joint] = self.velocities[joint];
            self.input.current_acceleration[joint] = self.accelerations[joint];
            self.input.target_position[joint] = position;
            self.input.target_velocity[joint] = velocity;
            self.input.target_acceleration[joint] = acceleration;
        }
        // INVARIANT: Current state comes from the guarded values above, so a clamp,
        // reset or followed motion also invalidates Ruckig's cached prediction.
        self.generator.delta_time = interval;
        match self.generator.update(&self.input, &mut self.output) {
            Ok(RuckigResult::Working | RuckigResult::Finished) => {}
            Ok(other) => {
                return Err(Trip::ReferenceFilter(format!(
                    "Reference filter failed: {other:?}"
                )));
            }
            Err(error) => {
                return Err(Trip::ReferenceFilter(format!(
                    "Reference filter failed: {error}"
                )));
            }
        }
        // CONSTRAINT: Past its end Ruckig extrapolates the target state, which is what a
        // reference that keeps moving at the target velocity would do.
        let positions = &mut self.output.new_position;
        let velocities = &self.output.new_velocity;
        let accelerations = &self.output.new_acceleration;
        // INVARIANT: Clamp only picoradian-scale numerical rounding at a boundary.
        for (joint, position) in positions.iter_mut().enumerate() {
            if *position < self.recovery_lower[joint] - ROUNDING_TOLERANCE_RADIANS
                || *position > self.recovery_upper[joint] + ROUNDING_TOLERANCE_RADIANS
            {
                return Err(Trip::ReferenceFilter(format!(
                    "Filtered reference left the joint limits: joint {joint}, position {position:.12}, bounds {:.12}..{:.12}, velocity {:.12}, prev {:.12}, ref {:.12}, target {:.12}, recovery {:?}",
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
        ReferenceFilter::new(&ControllerConfig::parse(TEST_CONFIG).unwrap(), start).unwrap()
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
        let config = ControllerConfig::parse(TEST_CONFIG).unwrap();
        assert!(ReferenceFilter::new(&config, &[2.5, 0.0]).is_err());
    }
}

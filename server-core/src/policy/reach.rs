use crate::arm::ReachGoal;
use nalgebra::Isometry3;

pub(crate) struct ActiveReach {
    goal: ReachGoal,
    settled_seconds: f64,
    elapsed: f64,
    reach_deadline_seconds: f64,
    settle_dwell_seconds: f64,
    settled_velocity_radians_per_second: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReachStatus {
    Running,
    Settled,
    TimedOut,
}

impl ActiveReach {
    pub(crate) fn new(goal: ReachGoal, settings: &crate::config::PolicyConfig) -> Self {
        Self {
            goal,
            settled_seconds: 0.0,
            elapsed: 0.0,
            reach_deadline_seconds: settings.reach_deadline_seconds,
            settle_dwell_seconds: settings.settle_dwell_seconds,
            settled_velocity_radians_per_second: settings.settled_velocity_radians_per_second,
        }
    }

    pub(crate) fn goal(&self) -> &ReachGoal {
        &self.goal
    }

    pub(crate) fn assess(
        &mut self,
        tool: &Isometry3<f64>,
        velocities: &[f64],
        interval: f64,
    ) -> ReachStatus {
        self.elapsed += interval;
        let distance =
            (tool.translation.vector - nalgebra::Vector3::from(self.goal.position)).norm();
        let rotation = tool.rotation;
        let alignment = (rotation.i * self.goal.quaternion[0]
            + rotation.j * self.goal.quaternion[1]
            + rotation.k * self.goal.quaternion[2]
            + rotation.w * self.goal.quaternion[3])
            .abs()
            .min(1.0);
        let reached = distance <= self.goal.position_tolerance_meters
            && 2.0 * alignment.acos() <= self.goal.orientation_tolerance_radians;
        let at_rest = velocities
            .iter()
            .all(|velocity| velocity.abs() <= self.settled_velocity_radians_per_second);
        self.settled_seconds = if reached && at_rest {
            self.settled_seconds + interval
        } else {
            0.0
        };
        if self.settled_seconds >= self.settle_dwell_seconds {
            ReachStatus::Settled
        } else if self.elapsed >= self.reach_deadline_seconds {
            ReachStatus::TimedOut
        } else {
            ReachStatus::Running
        }
    }
}

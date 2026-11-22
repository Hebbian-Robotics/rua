// INVARIANT: Learned-policy adaptation produces references and advances command state; hardware writes and setpoint guards remain in the common loop.

use super::activity::{Activity, Loop};
use crate::arm::{Now, ReachGoal};
use crate::kinematics::pose_from_xyzw;
use crate::limits::Trip;
use crate::plant::Feedback;
use crate::policy::{ActiveReach, ReachStatus};
use anyhow::Context;
use hebbian_contracts::CommandOutcome;

impl Loop<'_> {
    pub(super) fn start_reach(&mut self, goal: ReachGoal) -> anyhow::Result<()> {
        let runtime = self
            .policy
            .as_mut()
            .context("A pose goal arrived but no policy is loaded")?;
        runtime.reset();
        self.hold_at = None;
        self.activity = Activity::Reach(Box::new(ActiveReach::new(
            goal,
            self.config
                .policy
                .as_ref()
                .context("No policy execution settings")?,
        )));
        Ok(())
    }

    pub(super) fn reach_reference(
        &mut self,
        interval: f64,
        feedback: &Feedback,
        stop: bool,
        now: Now,
    ) -> Result<(), Trip> {
        if stop {
            self.end_stopped(now);
            return Ok(());
        }
        let policy_failed = |error: anyhow::Error| Trip(format!("Policy failed: {error:#}"));
        let Activity::Reach(active) = &mut self.activity else {
            return Err(Trip("No pose command is running".into()));
        };
        let runtime = self
            .policy
            .as_mut()
            .ok_or_else(|| Trip("No policy is loaded".into()))?;
        let goal = active.goal().clone();
        let tool = runtime.tool_pose(feedback).map_err(policy_failed)?;
        match active.assess(&tool, &feedback.velocities, interval) {
            ReachStatus::Running => {}
            ReachStatus::TimedOut => {
                self.end_timed_out(feedback, now);
                return Ok(());
            }
            ReachStatus::Settled => {
                self.filter.reset(&feedback.positions);
                self.end_activity(
                    CommandOutcome::Completed,
                    None,
                    Some(feedback.positions.clone()),
                    now,
                );
                return Ok(());
            }
        }
        let target = pose_from_xyzw(goal.position, goal.quaternion).map_err(policy_failed)?;
        let joint_goal = runtime
            .joint_goal(self.config, feedback, &target)
            .map_err(policy_failed)?;
        self.reference
            .set_at_rest(&joint_goal, Some(self.config.max_joint_speed));
        Ok(())
    }

    pub(super) fn policy_stream_reference(
        &mut self,
        feedback: &Feedback,
        position: [f64; 3],
        quaternion: [f64; 4],
    ) -> Result<(), Trip> {
        let policy_failed = |error: anyhow::Error| Trip(format!("Policy failed: {error:#}"));
        let runtime = self
            .policy
            .as_mut()
            .ok_or_else(|| Trip("A pose goal arrived but no policy is loaded".into()))?;
        if let Activity::Stream(stream) = &mut self.activity
            && !stream.policy_engaged
        {
            runtime.reset();
            stream.policy_engaged = true;
        }
        let target = pose_from_xyzw(position, quaternion).map_err(policy_failed)?;
        let goal = runtime
            .joint_goal(self.config, feedback, &target)
            .map_err(policy_failed)?;
        self.reference
            .set_at_rest(&goal, Some(self.config.max_joint_speed));
        Ok(())
    }
}

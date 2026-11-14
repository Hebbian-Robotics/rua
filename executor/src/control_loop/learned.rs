//! Live-loop adaptation for learned reaches and pose streams.
//! The policy module owns inference and settling, shared with preview. This
//! adapter updates command lifecycle and produces references; it never writes
//! hardware or bypasses the common setpoint guards.

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

    /// The reference for this cycle of a reach-and-finish command: the policy's
    /// step toward the pose, until the tool has settled within tolerance.
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
        let status = active
            .assess(runtime, feedback, interval)
            .map_err(policy_failed)?;
        let settled = status == ReachStatus::Settled;
        if status != ReachStatus::Running {
            if !settled {
                self.end_timed_out(feedback, now);
                return Ok(());
            }
            self.filter.reset(&feedback.positions);
            self.end_activity(
                CommandOutcome::Completed,
                None,
                Some(feedback.positions.clone()),
                now,
            );
            return Ok(());
        }
        let target = pose_from_xyzw(goal.position, goal.quaternion).map_err(policy_failed)?;
        let joint_goal = runtime
            .joint_goal(self.config, feedback, &target)
            .map_err(policy_failed)?;
        self.reference
            .set_at_rest(&joint_goal, Some(self.config.max_joint_speed));
        Ok(())
    }

    /// Produce the next reference for a live learned-policy pose stream.
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

// INVARIANT: Learned-policy adaptation produces references and advances command state; hardware writes and setpoint guards remain in the common loop.

use super::activity::{Activity, Loop};
use super::trajectory::MotionOutput;
use crate::arm::ReachGoal;
use crate::kinematics::pose_from_xyzw;
use crate::limits::Trip;
use crate::plant::Feedback;
use crate::policy::{ActiveReach, ReachStatus};
use anyhow::Context;
use hebbian_contracts::CommandOutcome;
use nalgebra::Isometry3;

pub(super) enum PolicySession {
    Continue,
    Reset,
}

pub(super) struct PolicyRequest {
    pub(super) session: PolicySession,
    pub(super) target: Isometry3<f64>,
}

pub(super) enum MotionPlan {
    ReferenceReady(MotionOutput),
    Infer(PolicyRequest),
}

impl PolicyRequest {
    pub(super) fn new(
        session: PolicySession,
        position: [f64; 3],
        quaternion: [f64; 4],
    ) -> Result<Self, Trip> {
        Ok(Self {
            session,
            target: pose_from_xyzw(position, quaternion).map_err(|error| policy_failure(&error))?,
        })
    }
}

pub(super) fn policy_failure(error: &anyhow::Error) -> Trip {
    Trip::Policy(format!("Policy failed: {error:#}"))
}

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
    ) -> Result<MotionPlan, Trip> {
        if stop {
            return Ok(MotionPlan::ReferenceReady(MotionOutput::Arm(
                self.prepare_stop_write(Vec::new(), "Stopped".into()),
            )));
        }
        let Activity::Reach(active) = &mut self.activity else {
            return Err(Trip::Policy("No pose command is running".into()));
        };
        let runtime = self
            .policy
            .as_ref()
            .ok_or_else(|| Trip::Policy("No policy is loaded".into()))?;
        let tool = runtime
            .tool_pose(feedback)
            .map_err(|error| policy_failure(&error))?;
        match active.assess(&tool, &feedback.velocities, interval) {
            ReachStatus::Running => {}
            ReachStatus::TimedOut => {
                let effects = self.prepare_measured_hold(
                    CommandOutcome::TimedOut,
                    Some("The pose was not reached before the deadline".into()),
                    feedback,
                );
                return Ok(MotionPlan::ReferenceReady(MotionOutput::Arm(effects)));
            }
            ReachStatus::Settled => {
                let effects = self.prepare_measured_hold(CommandOutcome::Completed, None, feedback);
                return Ok(MotionPlan::ReferenceReady(MotionOutput::Arm(effects)));
            }
        }
        let goal = active.goal();
        PolicyRequest::new(PolicySession::Continue, goal.position, goal.quaternion)
            .map(MotionPlan::Infer)
    }

    pub(super) fn adopt_policy_target(&mut self, joint_goal: &[f64]) -> MotionOutput {
        if let Activity::Stream(stream) = &mut self.activity {
            stream.policy_engaged = true;
        }
        self.reference
            .set_at_rest(joint_goal, Some(self.config.max_joint_speed));
        MotionOutput::None
    }
}

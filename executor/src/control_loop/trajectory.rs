//! Execution state and progress for planned IK/joint trajectories.
//! Grouped trajectories use the same progress handling; policy inference lives
//! separately in `learned`. Final setpoint guards stay in the common loop.

use super::activity::{Activity, Loop};
use crate::arm::{GroupMembership, Now};
use crate::corner_path::MotionSupervisor;
use crate::limits::{Trip, max_joint_deviation};
use crate::plant::Feedback;
use crate::reference_filter::Reference;
use hebbian_contracts::{Checkpoint, CommandOutcome, CornerPlan};

/// A planned motion being followed.
pub(super) struct ActiveMotion {
    pub(super) supervisor: MotionSupervisor,
    pub(super) checkpoints_sent: usize,
}

/// This arm's part of a grouped motion. The group's one trajectory is stepped
/// by the loop that runs every arm, not by the arm.
pub(super) struct ActiveGroupMember {
    pub(super) group: GroupMembership,
    pub(super) plan: CornerPlan,
}

/// Where a planned motion stands: still going, or ended where it was aimed or
/// stalled short of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MotionStatus {
    Running,
    Reached,
    StoppedShort,
}

impl MotionStatus {
    pub(super) fn of(supervisor: &MotionSupervisor) -> Self {
        match (supervisor.done(), supervisor.stopped_short()) {
            (false, _) => Self::Running,
            (true, false) => Self::Reached,
            (true, true) => Self::StoppedShort,
        }
    }
}

/// What one cycle of a planned motion did to its command.
pub(super) struct MotionProgress {
    /// Reached since the last cycle.
    pub(super) checkpoints: Vec<Checkpoint>,
    pub(super) segment: Option<String>,
    pub(super) status: MotionStatus,
    pub(super) final_pose: Vec<f64>,
}

/// One arm's share of a group's cycle: its reference, and what the group's
/// shared trajectory did.
#[derive(Clone)]
pub(super) struct GroupReference {
    pub(super) reference: Reference,
    pub(super) checkpoints: Vec<Checkpoint>,
    pub(super) segment: Option<String>,
    pub(super) status: MotionStatus,
    pub(super) final_pose: Vec<f64>,
}

impl Loop<'_> {
    /// Begin following a planned motion.
    pub(super) fn start_motion(&mut self, plan: CornerPlan) -> anyhow::Result<()> {
        let supervisor = MotionSupervisor::new(plan, &self.config.motion, self.config.joint_count)?;
        self.hold_at = None;
        self.activity = Activity::Motion(Box::new(ActiveMotion {
            supervisor,
            checkpoints_sent: 0,
        }));
        Ok(())
    }

    /// Begin this arm's part of a grouped motion; it follows the group's
    /// reference once every arm has begun.
    pub(super) fn start_group_member(&mut self, plan: CornerPlan, group: GroupMembership) {
        self.hold_at = None;
        self.activity = Activity::Group(Box::new(ActiveGroupMember { group, plan }));
    }

    /// The reference for this cycle of a running motion, left in `self.reference`.
    pub(super) fn motion_reference(
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
        if self.core.take_supersede() {
            // A new command replaced this one while it only settled; the arm holds
            // where it measures, and the new motion plans from there.
            self.filter.reset(&feedback.positions);
            self.hold_at = Some(feedback.positions.clone());
            self.core.superseded_motion_ended(&feedback.positions);
            self.activity = Activity::Idle;
            self.reference.set_at_rest(&feedback.positions, None);
            return Ok(());
        }
        let Activity::Motion(active) = &mut self.activity else {
            return Err(Trip("No motion is running".into()));
        };
        active.supervisor.advance(&feedback.positions, interval)?;
        self.reference.copy_from(active.supervisor.reference());
        let settling_at_end = active.supervisor.settling_at_end();
        let new = active
            .supervisor
            .take_new_checkpoints(active.checkpoints_sent);
        active.checkpoints_sent += new.len();
        let status = MotionStatus::of(&active.supervisor);
        let progress = MotionProgress {
            checkpoints: new,
            segment: active.supervisor.segment_label().map(str::to_owned),
            status,
            // Only a finished motion holds at its final pose.
            final_pose: if status == MotionStatus::Running {
                Vec::new()
            } else {
                active.supervisor.final_pose().to_vec()
            },
        };
        self.core.set_settling(settling_at_end);
        self.motion_progress(feedback, progress, now)
    }

    /// What a cycle of a planned motion, alone or in a group, did to the command:
    /// its checkpoints and segment, and whether it finished.
    /// Leaves the cycle's reference in `self.reference`.
    pub(super) fn motion_progress(
        &mut self,
        feedback: &Feedback,
        progress: MotionProgress,
        now: Now,
    ) -> Result<(), Trip> {
        for checkpoint in progress.checkpoints {
            self.core.add_checkpoint(checkpoint);
        }
        self.core.set_segment(progress.segment);
        if progress.status != MotionStatus::Running {
            // The last rest point may have stalled short; the outcome says so and
            // the record carries each joint's error.
            let (outcome, detail) = if progress.status == MotionStatus::StoppedShort {
                let errors: Vec<String> = feedback
                    .positions
                    .iter()
                    .zip(&self.reference.positions)
                    .map(|(measured, target)| format!("{:.3}", (measured - target).abs()))
                    .collect();
                (
                    CommandOutcome::StoppedShort,
                    format!(
                        "Stopped short; final joint errors (rad): [{}]",
                        errors.join(", ")
                    ),
                )
            } else {
                let final_error =
                    max_joint_deviation(&feedback.positions, &self.reference.positions);
                (
                    CommandOutcome::Completed,
                    format!("Final joint error {final_error:.4} rad"),
                )
            };
            self.end_activity(outcome, Some(detail), Some(progress.final_pose), now);
            return Ok(());
        }
        Ok(())
    }

    /// The reference for this cycle of this arm's part of a grouped motion, as
    /// the group's shared trajectory gave it; until the group has formed, hold.
    pub(super) fn group_reference(
        &mut self,
        feedback: &Feedback,
        stop: bool,
        group: Option<&Result<GroupReference, Trip>>,
        now: Now,
    ) -> Result<(), Trip> {
        if stop {
            self.end_stopped(now);
            return Ok(());
        }
        let Some(share) = group else {
            self.reference.set_at_rest(self.filter.positions(), None);
            return Ok(());
        };
        let share = share.as_ref().map_err(Clone::clone)?;
        self.reference.copy_from(&share.reference);
        self.motion_progress(
            feedback,
            MotionProgress {
                checkpoints: share.checkpoints.clone(),
                segment: share.segment.clone(),
                status: share.status,
                final_pose: share.final_pose.clone(),
            },
            now,
        )
    }
}

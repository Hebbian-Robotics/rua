// INVARIANT: Pure motion decisions. Record mutation and event publication happen only
// after a decision succeeds, in `ArmCore`'s imperative admission shell.

use super::types::ShutdownState;
use super::types::rejection;
use super::{ArmCore, Limits, Rejection};
use crate::corner_path::ValidatedPlan;
use crate::motion_values::JointSpeed;
use hebbian_contracts::{CornerPlan, RejectionReason, Segment};

// CONSTRAINT: Geometry admitted against one arm's bounds and speed cap, rebased to its
// current start. Collision and physical tracking checks remain with the planner
// and per-cycle guards; this type does not claim those observations stay valid.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AdmittedPlan(ValidatedPlan);

impl AdmittedPlan {
    fn new(mut plan: ValidatedPlan, limits: &Limits, start: Vec<f64>) -> anyhow::Result<Self> {
        plan.rebase(start)?;
        anyhow::ensure!(
            plan.peak_speed <= limits.max_joint_speed,
            "Path speed exceeds the arm's maximum"
        );
        anyhow::ensure!(
            limits.lower.len() == limits.joint_count && limits.upper.len() == limits.joint_count,
            "Arm limits do not match its joint count"
        );
        for (segment_index, segment) in plan.segments.iter().enumerate() {
            for (corner_index, corner) in segment.corners.iter().enumerate() {
                let tolerance = if segment_index == 0 && corner_index == 0 {
                    limits.joint_limit_recovery_tolerance_radians
                } else {
                    0.0
                };
                for (joint, ((value, lower), upper)) in corner
                    .iter()
                    .zip(&limits.lower)
                    .zip(&limits.upper)
                    .enumerate()
                {
                    anyhow::ensure!(
                        *value >= lower - tolerance && *value <= upper + tolerance,
                        "Path joint {joint} is outside the arm's limits"
                    );
                }
            }
        }
        Ok(Self(plan))
    }

    pub(crate) fn into_validated(self) -> ValidatedPlan {
        self.0
    }
}

impl std::ops::Deref for AdmittedPlan {
    type Target = ValidatedPlan;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl ArmCore {
    pub(crate) fn shutdown_return_plan(
        &self,
        home_target: Vec<f64>,
    ) -> anyhow::Result<AdmittedPlan> {
        anyhow::ensure!(
            self.shutdown == ShutdownState::ReturningToRest,
            "The arm is not returning to rest for shutdown"
        );
        let poses = self.poses().ok_or_else(|| anyhow::anyhow!("Not holding"))?;
        let plan = CornerPlan {
            segments: vec![Segment {
                label: "return_to_rest".into(),
                corners: vec![poses.hold.clone(), home_target],
                minimum_duration_seconds: None,
                target_position: None,
                target_quaternion: None,
            }],
            peak_speed: self.limits.default_joint_speed,
        };
        let plan = ValidatedPlan::new(plan, self.limits.joint_count)?;
        AdmittedPlan::new(plan, &self.limits, poses.hold.clone())
    }
}

pub(crate) enum MoveDisposition {
    Queue,
    ReplaceSettling { command_id: String },
}

#[derive(Clone, Copy)]
pub(crate) enum MotionScope {
    Individual,
    Grouped,
}

pub(crate) struct MoveDecision {
    pub(crate) plan: AdmittedPlan,
    pub(crate) speed: JointSpeed,
    pub(crate) disposition: MoveDisposition,
}

impl MoveDecision {
    pub(crate) fn prepare(
        core: &ArmCore,
        plan: CornerPlan,
        speed: f64,
        scope: MotionScope,
    ) -> Result<Self, Rejection> {
        let invalid =
            |error: anyhow::Error| rejection(RejectionReason::InvalidTarget, format!("{error:#}"));
        let replacing = matches!(scope, MotionScope::Individual) && core.replaces_settling_motion();
        if !replacing && let Some(refused) = core.mode_rejection() {
            return Err(refused);
        }
        let poses = core
            .poses()
            .ok_or_else(|| rejection(RejectionReason::NotEnabled, "Not holding"))?;
        let speed = JointSpeed::new(speed).map_err(invalid)?;
        if speed.radians_per_second() > core.limits.max_joint_speed {
            return Err(rejection(
                RejectionReason::InvalidTarget,
                "Joint speed exceeds the arm's maximum",
            ));
        }
        let plan = ValidatedPlan::new(plan, core.limits.joint_count).map_err(invalid)?;
        if plan.peak_speed.to_bits() != speed.radians_per_second().to_bits() {
            return Err(rejection(
                RejectionReason::InvalidTarget,
                "Move speed must match path peak speed",
            ));
        }
        if matches!(scope, MotionScope::Grouped)
            && (plan.segments.len() != 1 || plan.segments[0].corners.len() != 2)
        {
            return Err(rejection(
                RejectionReason::InvalidTarget,
                "A grouped move needs one straight section",
            ));
        }
        let start = if replacing {
            core.observation
                .as_ref()
                .map_or(&poses.hold, |observed| &observed.positions)
        } else {
            &poses.hold
        };
        let plan = AdmittedPlan::new(plan, &core.limits, start.clone()).map_err(invalid)?;
        let disposition = if replacing {
            let command_id = core.active_command_id().ok_or_else(|| {
                rejection(
                    RejectionReason::Faulted,
                    "A settling motion has no active command",
                )
            })?;
            MoveDisposition::ReplaceSettling {
                command_id: command_id.to_owned(),
            }
        } else {
            MoveDisposition::Queue
        };
        Ok(Self {
            plan,
            speed,
            disposition,
        })
    }
}

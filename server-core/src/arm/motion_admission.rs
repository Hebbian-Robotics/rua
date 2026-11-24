#![deny(clippy::disallowed_methods)]

// INVARIANT: Pure motion decisions. Record mutation and event publication happen only
// after a decision succeeds, in `ArmCore`'s imperative admission shell.

use super::{GripperTarget, Limits, Rejection, rejection};
use crate::corner_path::ValidatedPlan;
use crate::motion_values::JointSpeed;
use hebbian_contracts::{CornerPlan, JawAngle, RejectionReason};

// CONSTRAINT: Geometry admitted against one arm's bounds and speed cap, rebased to its
// current start. Collision and physical tracking checks remain with the planner
// and per-cycle guards; this type does not claim those observations stay valid.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AdmittedPlan(ValidatedPlan);

impl AdmittedPlan {
    pub(super) fn new(
        mut plan: ValidatedPlan,
        limits: &Limits,
        start: Vec<f64>,
    ) -> anyhow::Result<Self> {
        plan.rebase(start)?;
        anyhow::ensure!(
            plan.peak_speed <= limits.max_joint_speed,
            "Path speed exceeds the arm's maximum"
        );
        for (segment_index, segment) in plan.segments.iter().enumerate() {
            for (corner_index, corner) in segment.corners.iter().enumerate() {
                let tolerance = if segment_index == 0 && corner_index == 0 {
                    limits.joint_bounds.tolerance()
                } else {
                    0.0
                };
                for (joint, ((value, lower), upper)) in corner
                    .iter()
                    .zip(limits.joint_bounds.lower())
                    .zip(limits.joint_bounds.upper())
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

pub(crate) enum MoveDisposition {
    Queue,
    ReplaceSettling {
        command_id: hebbian_contracts::CommandId,
    },
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
    pub(super) fn prepare(
        plan: CornerPlan,
        speed: JointSpeed,
        scope: MotionScope,
        limits: &Limits,
        start: &[f64],
        disposition: MoveDisposition,
    ) -> Result<Self, Rejection> {
        let invalid =
            |error: anyhow::Error| rejection(RejectionReason::InvalidTarget, format!("{error:#}"));
        if speed.radians_per_second() > limits.max_joint_speed {
            return Err(rejection(
                RejectionReason::InvalidTarget,
                "Joint speed exceeds the arm's maximum",
            ));
        }
        let plan = ValidatedPlan::new(plan, limits.joint_bounds.lower().len()).map_err(invalid)?;
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
        if let Some(angle) = plan.final_segment().final_gripper_angle {
            if matches!(scope, MotionScope::Grouped) || plan.is_continuous() {
                return Err(rejection(
                    RejectionReason::InvalidTarget,
                    "A final gripper target needs an individual move with stops",
                ));
            }
            gripper_target(limits, angle, Some(speed), true)?;
        }
        for angle in plan
            .segments
            .iter()
            .filter_map(|segment| segment.pass_through?.gripper_angle)
        {
            gripper_target(limits, angle, Some(speed), true)?;
        }
        let plan = AdmittedPlan::new(plan, limits, start.to_vec()).map_err(invalid)?;
        Ok(Self {
            plan,
            speed,
            disposition,
        })
    }
}

pub(super) fn gripper_target(
    limits: &Limits,
    angle: JawAngle,
    requested_speed: Option<JointSpeed>,
    holding: bool,
) -> Result<GripperTarget, Rejection> {
    let Some((minimum, maximum)) = limits.gripper else {
        return Err(rejection(
            RejectionReason::InvalidTarget,
            "This arm has no gripper",
        ));
    };
    if !holding {
        return Err(rejection(
            RejectionReason::NotEnabled,
            "The arm is disabled; send enable, then repeat the command",
        ));
    }
    let angle = JawAngle::try_from(angle.radians().clamp(minimum, maximum))
        .map_err(|error| rejection(RejectionReason::InvalidTarget, error.to_string()))?;
    let speed = JointSpeed::for_request(
        requested_speed,
        limits.default_joint_speed,
        limits.max_joint_speed,
    )
    .map_err(|invalid| rejection(RejectionReason::InvalidTarget, invalid.to_string()))?;
    Ok(GripperTarget { angle, speed })
}

use super::activity::Loop;
use super::trajectory::MotionOutput;
use crate::gripper::GripperState;
use crate::limits::Trip;
use crate::plant::Feedback;
use hebbian_contracts::GripperResult;

pub(super) struct GripperPlan {
    gripper: GripperEffect,
    trip: Option<String>,
}

pub(super) struct CycleOutputs {
    pub(super) motion: MotionOutput,
    pub(super) gripper: GripperEffect,
    pub(super) trip: Option<String>,
}

pub(super) struct GripperWrite {
    pub(super) angle: f64,
    pub(super) completed: Option<GripperResult>,
}

pub(super) enum GripperEffect {
    Idle,
    Write(GripperWrite),
    Refused(String),
    Failed { detail: String, hold: Option<f64> },
}

impl GripperEffect {
    pub(super) fn setpoint(&self) -> Option<f64> {
        match self {
            Self::Idle | Self::Refused(_) => None,
            Self::Write(write) => Some(write.angle),
            Self::Failed { hold, .. } => *hold,
        }
    }
}

impl Loop<'_> {
    pub(super) fn plan_gripper(
        &mut self,
        feedback: &Feedback,
        interval: f64,
        feedback_failure: Option<Trip>,
    ) -> GripperPlan {
        let mut trip = feedback_failure
            .filter(|_| self.tripped.is_none())
            .map(|trip| self.prepare_trip(trip, feedback));
        let gripper = match self.plan_gripper_write(feedback, interval) {
            Ok(gripper) => gripper,
            Err(failure) => {
                let detail = format!("Gripper control failed: {failure:#}");
                trip.get_or_insert_with(|| {
                    self.prepare_trip(Trip::Gripper(detail.clone()), feedback)
                });
                if let Some(measured) = feedback.gripper_angle
                    && let Some(jaw) = &mut self.gripper
                {
                    jaw.hold(measured);
                }
                GripperEffect::Failed {
                    detail,
                    hold: feedback.gripper_angle,
                }
            }
        };
        GripperPlan { gripper, trip }
    }

    pub(super) fn finish_cycle_plan(
        &mut self,
        motion: Result<MotionOutput, Trip>,
        GripperPlan {
            mut gripper,
            mut trip,
        }: GripperPlan,
        feedback: &Feedback,
        interval: f64,
    ) -> CycleOutputs {
        let motion = match motion.and_then(|effects| {
            self.guard_setpoint(&effects, interval, feedback)?;
            Ok(effects)
        }) {
            Ok(effects) => effects,
            Err(failure) => {
                trip = Some(self.prepare_trip(failure, feedback));
                self.next_command.clone_from(&feedback.positions);
                if matches!(gripper, GripperEffect::Write(_))
                    && let Some(measured) = feedback.gripper_angle
                    && let Some(jaw) = &mut self.gripper
                {
                    jaw.hold(measured);
                    gripper =
                        GripperEffect::Refused("The arm tripped before the gripper write".into());
                }
                MotionOutput::None
            }
        };
        CycleOutputs {
            motion,
            gripper,
            trip,
        }
    }

    fn plan_gripper_write(
        &mut self,
        feedback: &Feedback,
        interval: f64,
    ) -> anyhow::Result<GripperEffect> {
        let (Some(measured), Some(_)) = (feedback.gripper_angle, self.gripper.as_ref()) else {
            if self.powered() && self.config.gripper.is_some() {
                return Err(anyhow::anyhow!(
                    "Missing measurement or controller for the configured gripper"
                ));
            }
            return Ok(GripperEffect::Idle);
        };
        if self.arm().take_gripper_stop() {
            // INVARIANT: The record already ended stopped; the jaw holds where it is.
            self.gripper_command = None;
            if let Some(jaw) = &mut self.gripper {
                jaw.hold(measured);
            }
        }
        let new_target = self.arm().take_gripper_target();
        if let Some((command_id, gripper_target)) = new_target {
            if let Some(jaw) = &mut self.gripper {
                jaw.set_target(
                    gripper_target.angle.radians(),
                    gripper_target.speed.radians_per_second(),
                    measured,
                )?;
            }
            self.gripper_command = Some(command_id);
        }
        let Some(jaw) = &mut self.gripper else {
            return Ok(GripperEffect::Idle);
        };
        let proposed_command = jaw.step(measured, interval)?;
        let target = jaw.target();
        // WHY: Check only this cycle's smoothed travel, rather than scanning the
        // entire opening in one motor cycle. Recheck at the current arm pose
        // while arm and jaw move together.
        let command = match self.safe_jaw_target(proposed_command, measured, feedback) {
            Ok(command) => command,
            Err(reason) => {
                if let Some(jaw) = &mut self.gripper {
                    jaw.hold(measured);
                }
                return Ok(GripperEffect::Refused(reason));
            }
        };
        let jaw = self.gripper.as_mut().ok_or_else(|| {
            anyhow::anyhow!("The active gripper controller disappeared during its cycle")
        })?;
        let clipped = (command - proposed_command).abs() > 1e-8;
        if clipped {
            jaw.hold(command);
        }
        let state = jaw.state();
        let completed = match (state, clipped) {
            (GripperState::Moving, _) => None,
            (GripperState::Reached, false) => Some(GripperResult::Reached {
                measured_angle_radians: measured,
            }),
            _ => Some(GripperResult::StoppedShort {
                measured_angle_radians: measured,
                target_angle_radians: target,
            }),
        };
        Ok(GripperEffect::Write(GripperWrite {
            angle: command,
            completed,
        }))
    }
}

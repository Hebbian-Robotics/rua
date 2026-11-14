//! Smooth jaw motion and contact detection. The motor firmware caps grip force.

use crate::config::{GripperLimits, MotionLimits};
use crate::corner_path::OnlineSection;
use crate::reference_filter::Reference;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GripperState {
    Moving,
    Reached,
    /// Stopped short of the target: closed on something, or blocked opening.
    Contact,
}

pub(crate) struct GripperController {
    tolerance: f64,
    contact_dwell_seconds: f64,
    contact_detection_dwell_seconds: f64,
    requested_speed: Option<f64>,
    motion_limits: MotionLimits,
    target: f64,
    reference: Reference,
    trajectory: Option<OnlineSection>,
    state: GripperState,
    still_at: f64,
    still_seconds: f64,
}

impl GripperController {
    pub fn new(limits: &GripperLimits, motion_limits: &MotionLimits, position: f64) -> Self {
        Self {
            tolerance: limits.position_tolerance_radians,
            contact_dwell_seconds: limits.contact_dwell_seconds,
            contact_detection_dwell_seconds: limits.contact_dwell_seconds,
            requested_speed: None,
            motion_limits: motion_limits.clone(),
            target: position,
            reference: Reference::at_rest(&[position], None),
            trajectory: None,
            state: GripperState::Reached,
            still_at: position,
            still_seconds: 0.0,
        }
    }

    pub(crate) fn set_target(&mut self, angle: f64, speed: f64, measured: f64) {
        let same_target = (angle - self.target).abs() <= 1e-8;
        let same_speed = self
            .requested_speed
            .is_some_and(|previous_speed| (previous_speed - speed).abs() <= f64::EPSILON);
        let contact_direction = (self.target - measured) * (angle - measured);
        if self.state == GripperState::Contact && contact_direction >= 0.0 {
            // Repeated or refined goals into an obstruction must not restart
            // closing. A goal on the other side of the jaw releases it.
            if contact_direction > 0.0 {
                self.target = angle;
            }
            return;
        }
        if same_target && self.state == GripperState::Moving && same_speed {
            return;
        }
        // A stream may refine its target every cycle. Keep watching for a
        // stalled jaw while those targets continue in the same direction.
        let reset_contact_detection = self.state != GripperState::Moving
            || (self.target - measured) * (angle - measured) <= 0.0;
        self.target = angle;
        self.requested_speed = Some(speed);
        // A slowly commanded jaw needs time to travel beyond the feedback
        // tolerance; otherwise quantized feedback can look like a stall.
        self.contact_detection_dwell_seconds =
            self.contact_dwell_seconds.max(2.0 * self.tolerance / speed);
        self.trajectory = if (angle - self.reference.positions[0]).abs() > 1e-8 {
            Some(OnlineSection::new(
                &self.reference,
                &[angle],
                speed,
                &self.motion_limits,
            ))
        } else {
            None
        };
        self.state = GripperState::Moving;
        if reset_contact_detection {
            self.still_at = measured;
            self.still_seconds = 0.0;
        }
    }

    pub(crate) fn hold(&mut self, measured: f64) {
        self.target = measured;
        self.requested_speed = None;
        self.reference = Reference::at_rest(&[measured], None);
        self.trajectory = None;
        self.state = GripperState::Reached;
    }

    pub(crate) fn state(&self) -> GripperState {
        self.state
    }
    pub(crate) fn target(&self) -> f64 {
        self.target
    }

    pub fn step(&mut self, measured: f64, interval: f64) -> anyhow::Result<f64> {
        if self.state == GripperState::Moving {
            if let Some(trajectory) = &mut self.trajectory {
                self.reference = trajectory.step(interval, None)?;
            }
            let command = self.reference.positions[0];
            if (command - self.target).abs() <= 1e-8
                && (measured - self.target).abs() <= self.tolerance
            {
                self.state = GripperState::Reached;
                self.trajectory = None;
            } else if (command - measured).abs() > self.tolerance {
                if (measured - self.still_at).abs() > self.tolerance {
                    self.still_at = measured;
                    self.still_seconds = 0.0;
                } else {
                    self.still_seconds += interval;
                    if self.still_seconds >= self.contact_detection_dwell_seconds {
                        // Hold only one position tolerance past the measured jaw. This
                        // keeps the force-capped servo engaged without retaining a large
                        // position error accumulated while the jaw was blocked.
                        let holding_command =
                            measured + (command - measured).clamp(-self.tolerance, self.tolerance);
                        self.reference = Reference::at_rest(&[holding_command], None);
                        self.trajectory = None;
                        self.state = GripperState::Contact;
                    }
                }
            } else {
                self.still_at = measured;
                self.still_seconds = 0.0;
            }
        }
        Ok(self.reference.positions[0])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn controller(position: f64) -> GripperController {
        GripperController::new(
            &GripperLimits {
                closed_radians: 0.0,
                open_radians: 1.5,
                position_tolerance_radians: 0.01,
                contact_dwell_seconds: 0.2,
            },
            &MotionLimits {
                acceleration_limit: 2.0,
                jerk_limit: 10.0,
                settled_error_radians: 0.02,
                settle_dwell_seconds: 0.1,
                settle_timeout_seconds: 2.0,
            },
            position,
        )
    }

    #[test]
    fn jaw_accelerates_brakes_and_reaches_the_target() {
        let mut jaw = controller(0.0);
        jaw.set_target(1.0, 1.0, 0.0);
        let mut measured = 0.0;
        let mut velocities = Vec::new();
        while jaw.state() == GripperState::Moving {
            let command = jaw.step(measured, 0.01).unwrap();
            velocities.push((command - measured) / 0.01);
            measured = command;
            assert!(velocities.len() < 1000);
        }
        assert_eq!(jaw.state(), GripperState::Reached);
        assert!((measured - 1.0).abs() < 1e-6);
        assert!(velocities.iter().all(|speed| *speed <= 1.001));
        assert!(velocities[0] < 0.1);
        assert!(velocities.last().unwrap() < &0.1);
        assert!(velocities.iter().any(|speed| *speed > 0.5));
    }

    #[test]
    fn contact_freezes_the_command_before_the_far_endpoint_and_allows_reopening() {
        let mut jaw = controller(1.0);
        jaw.set_target(0.0, 1.0, 1.0);
        let mut measured = 1.0;
        for _ in 0..500 {
            let command = jaw.step(measured, 0.01).unwrap();
            measured = command.max(0.6);
            if jaw.state() == GripperState::Contact {
                break;
            }
        }
        assert_eq!(jaw.state(), GripperState::Contact);
        let held = jaw.step(measured, 0.01).unwrap();
        assert!(held > 0.0 && held < measured);
        assert!(measured - held <= 0.010_000_1);
        for _ in 0..20 {
            assert_eq!(jaw.step(measured, 0.01).unwrap(), held);
        }
        jaw.set_target(1.0, 1.0, measured);
        for _ in 0..500 {
            measured = jaw.step(measured, 0.01).unwrap();
            if jaw.state() == GripperState::Reached {
                break;
            }
        }
        assert_eq!(jaw.state(), GripperState::Reached);
    }

    #[test]
    fn repeated_stream_targets_do_not_restart_contact_detection_or_closing() {
        let mut jaw = controller(1.0);
        let mut measured = 1.0;
        for _ in 0..500 {
            jaw.set_target(0.0, 1.0, measured);
            let command = jaw.step(measured, 0.01).unwrap();
            measured = command.max(0.6);
            if jaw.state() == GripperState::Contact {
                break;
            }
        }
        assert_eq!(jaw.state(), GripperState::Contact);
        let held = jaw.step(measured, 0.01).unwrap();
        for _ in 0..20 {
            jaw.set_target(0.0, 1.0, measured);
            assert_eq!(jaw.state(), GripperState::Contact);
            assert_eq!(jaw.step(measured, 0.01).unwrap(), held);
        }
    }

    #[test]
    fn changing_stream_targets_in_one_direction_do_not_hide_contact() {
        let mut jaw = controller(1.0);
        let mut measured = 1.0;
        for index in 0..500 {
            let target = if index % 2 == 0 { 0.0 } else { 0.05 };
            jaw.set_target(target, 1.0, measured);
            let command = jaw.step(measured, 0.01).unwrap();
            measured = command.max(0.6);
            if jaw.state() == GripperState::Contact {
                break;
            }
        }
        assert_eq!(jaw.state(), GripperState::Contact);
        let held = jaw.step(measured, 0.01).unwrap();
        jaw.set_target(0.0, 1.0, measured);
        jaw.set_target(0.05, 1.0, measured);
        assert_eq!(jaw.state(), GripperState::Contact);
        assert_eq!(jaw.step(measured, 0.01).unwrap(), held);
    }

    #[test]
    fn a_moving_jaw_can_reverse_with_a_lower_speed_limit() {
        let mut jaw = controller(0.0);
        jaw.set_target(1.0, 1.0, 0.0);
        let mut measured = 0.0;
        for _ in 0..100 {
            measured = jaw.step(measured, 0.01).unwrap();
        }
        jaw.set_target(0.0, 0.2, measured);
        for _ in 0..1500 {
            measured = jaw.step(measured, 0.01).unwrap();
            if jaw.state() != GripperState::Moving {
                break;
            }
        }
        assert_eq!(jaw.state(), GripperState::Reached);
        assert!(measured.abs() < 1e-6);
    }

    #[test]
    fn a_slow_jaw_is_not_mistaken_for_contact() {
        let mut jaw = controller(0.0);
        jaw.set_target(0.05, 0.01, 0.0);
        let mut measured = 0.0;
        for _ in 0..1000 {
            measured = jaw.step(measured, 0.01).unwrap();
            if jaw.state() != GripperState::Moving {
                break;
            }
        }
        assert_eq!(jaw.state(), GripperState::Reached);
    }
}

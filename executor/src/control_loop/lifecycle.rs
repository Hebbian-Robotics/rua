//! Enabling, releasing, carrying out queued commands and shutting down.

use super::runner::Runner;
use crate::arm::{Job, Now};
use crate::joint_limits::{OutsideRecovery, clamp_within_recovery};
use crate::limits::max_joint_deviation;
use crate::plant::{Feedback, Plant};
use hebbian_contracts::{CornerPlan, Segment};
/// On shutdown, an arm this close to its rest pose is released without moving.
const SHUTDOWN_AT_REST_RADIANS: f64 = 0.01;

impl<P: Plant> Runner<'_, P> {
    /// The preflight, the drift checks and torque on; the reading that confirms it.
    pub(super) fn enable_checked(&mut self) -> anyhow::Result<Feedback> {
        let config = self.config;
        self.plant.preflight()?;
        let initial = self.plant.read_feedback()?;
        if let Err(OutsideRecovery { joint, value }) = clamp_within_recovery(
            &initial.positions,
            &config.lower_limits,
            &config.upper_limits,
            config.joint_limit_recovery_tolerance_radians,
        ) {
            anyhow::bail!(
                "Joint {joint} at {value:.3} rad is outside its limits; move it inside by hand before enabling"
            );
        }
        // The guard watches from before torque goes on.
        self.plant.start_guard()?;
        if let Some(checks) = &config.enable_checks {
            let settled = self.plant.read_feedback()?;
            anyhow::ensure!(
                max_joint_deviation(&settled.positions, &initial.positions)
                    <= checks.preflight_drift_limit_radians,
                "Moved during preflight"
            );
        }
        self.plant.enable()?;
        let fresh = self.plant.read_feedback()?;
        if let Some(checks) = &config.enable_checks {
            anyhow::ensure!(
                max_joint_deviation(&fresh.positions, &initial.positions)
                    <= checks.enable_drift_limit_radians,
                "Moved during enable"
            );
        }
        if let Some(fault) = &fresh.fault {
            anyhow::bail!("Enable not confirmed: {fault}");
        }
        Ok(fresh)
    }

    /// Take the arm's torque up, holding the present pose.
    pub(super) fn enable(&mut self, now: Now) {
        let result = self.enable_checked();
        match result {
            Ok(fresh) => {
                self.state.power_up(&fresh.positions, fresh.gripper_angle);
                self.state.arm().enabled(&fresh.positions, now);
            }
            Err(reason) => {
                // A refused preflight leaves the arm disabled; release what the
                // attempt may have left on.
                let _ = self.plant.release();
                self.state
                    .arm()
                    .failed(&format!("Enable failed: {reason:#}"), now);
            }
        }
    }

    pub(super) fn release(&mut self, now: Now) {
        let cleanup_errors: Vec<String> = self
            .plant
            .release()
            .err()
            .map(|error| format!("{error:#}"))
            .into_iter()
            .collect();
        self.state.power_down();
        self.state.arm().released(&cleanup_errors, now);
    }

    /// Carry out the next queued command, if one waits; true if it took longer
    /// than a cycle, so the cycle timer must start afresh.
    pub(super) fn start_next(&mut self, now: Now) -> bool {
        let (job, mode) = {
            let arm = self.state.arm();
            (arm.next_job(), arm.mode())
        };
        let Some(job) = job else {
            return self.shutdown_step(now);
        };
        match job {
            Job::ApproveRecovery { .. } => {
                self.state
                    .arm()
                    .failed("The arm is not waiting for recovery approval", now);
                false
            }
            Job::Enable { .. } => {
                if mode.accepts_enable() {
                    self.enable(now);
                } else {
                    self.state.arm().failed("The arm is already enabled", now);
                }
                true
            }
            Job::Release { .. } => {
                if self.state.powered {
                    self.release(now);
                } else {
                    self.state.arm().failed(
                        "The arm is not enabled; send enable, then repeat the command",
                        now,
                    );
                }
                true
            }
            Job::Move { .. } | Job::Stream { .. } | Job::Reach { .. } if !self.state.powered => {
                self.state.arm().failed(
                    "The arm is not enabled; send enable, then repeat the command",
                    now,
                );
                false
            }
            Job::Move { .. } | Job::Stream { .. } | Job::Reach { .. }
                if self.state.tripped.is_some() =>
            {
                let reason = self.state.tripped.clone().unwrap_or_default();
                self.state
                    .arm()
                    .failed(&format!("The arm is tripped: {reason}"), now);
                false
            }
            Job::Move {
                plan,
                group: Some(group),
                ..
            } => {
                self.state.start_group_member(plan, group);
                self.state.arm().begin_moving();
                false
            }
            Job::Move { plan, .. } => {
                match self.state.start_motion(plan) {
                    Ok(()) => self.state.arm().begin_moving(),
                    Err(reason) => self.state.arm().failed(&format!("{reason:#}"), now),
                }
                false
            }
            Job::Reach { goal, .. } => {
                match self.state.start_reach(goal) {
                    Ok(()) => self.state.arm().begin_moving(),
                    Err(reason) => self.state.arm().failed(&format!("{reason:#}"), now),
                }
                false
            }
            Job::Stream { command_id } => {
                self.state.start_stream(command_id);
                self.state.arm().begin_moving();
                false
            }
        }
    }

    /// On shutdown, with nothing left to carry out: return to where the arm
    /// rested unpowered, then release.
    pub(super) fn shutdown_step(&mut self, now: Now) -> bool {
        if !self.state.arm().shutdown_requested() || !self.state.powered {
            return false;
        }
        let poses = self.state.arm().poses().cloned();
        let Some(poses) = poses else {
            self.release(now);
            return true;
        };
        let measured = self.state.previous_command.clone();
        let at_rest = max_joint_deviation(&measured, &poses.rest) <= SHUTDOWN_AT_REST_RADIANS;
        if at_rest || self.state.shutdown_return_attempted {
            self.release(now);
            return true;
        }
        self.state.shutdown_return_attempted = true;
        self.state.arm().allow_shutdown_return();
        let plan = CornerPlan {
            segments: vec![Segment {
                label: "return_to_rest".into(),
                corners: vec![poses.hold.clone(), poses.rest.clone()],
                minimum_duration_seconds: None,
                target_position: None,
                target_quaternion: None,
            }],
            peak_speed: self.config.default_joint_speed,
        };
        match self.state.start_motion(plan) {
            Ok(()) => self.state.arm().begin_moving(),
            Err(_) => self.release(now),
        }
        false
    }
}

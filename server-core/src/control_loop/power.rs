use std::time::Instant;

use super::runner::Runner;
use crate::arm::Now;
use crate::limits::{check_effort, max_joint_deviation};
use crate::plant::{Feedback, Pacing, Plant};
use hebbian_robot_geometry::joint_limits::RecoveryRefusal;
const STARTUP_HOLD_SECONDS: f64 = 0.25;
const SHUTDOWN_AT_REST_RADIANS: f64 = 0.01;

impl<P: Plant> Runner<'_, P> {
    pub(super) fn enable_checked(&mut self) -> anyhow::Result<Feedback> {
        let config = self.config;
        self.plant.preflight()?;
        let initial = self.plant.read_feedback()?;
        initial.measured_positions(config)?;
        if let Err(error) = config.recovery_limits().clamp(&initial.positions) {
            return Err(match error {
                RecoveryRefusal::OutsideRecovery { .. } => anyhow::anyhow!(
                    "{error}; move the arm inside its limits by hand before enabling"
                ),
                error => anyhow::anyhow!(error),
            });
        }
        // INVARIANT: The guard watches from before torque goes on.
        self.plant.start_guard()?;
        if let Some(checks) = &config.enable_checks {
            let settled = self.plant.read_feedback()?;
            let settled_positions = settled.measured_positions(config)?;
            anyhow::ensure!(
                max_joint_deviation(settled_positions.as_slice(), &initial.positions)
                    <= checks.preflight_drift_limit_radians,
                "Moved during preflight"
            );
        }
        self.plant.enable()?;
        let period_seconds = 1.0 / config.frequency_hertz;
        let period = config.control_timing.period;
        let observation_cycles = (STARTUP_HOLD_SECONDS * config.frequency_hertz).ceil();
        let mut observed_cycles = 0.0;
        loop {
            let cycle_started = Instant::now();
            let fresh = self.plant.read_feedback()?;
            let fresh_positions = fresh.measured_positions(config)?;
            if let Some(checks) = &config.enable_checks {
                anyhow::ensure!(
                    max_joint_deviation(fresh_positions.as_slice(), &initial.positions)
                        <= checks.enable_drift_limit_radians,
                    "Moved during enable"
                );
            }
            if let Some(fault) = &fresh.fault {
                anyhow::bail!("Enable not confirmed: {fault}");
            }
            check_effort(config, &fresh.efforts).map_err(|trip| anyhow::anyhow!(trip.0))?;
            if config.enable_checks.is_none() || observed_cycles >= observation_cycles {
                return Ok(fresh);
            }

            self.plant.heartbeat()?;
            self.plant.advance(period_seconds)?;
            if self.plant.pacing() != Pacing::RepliesAsFastAsPossible {
                std::thread::sleep(period.saturating_sub(cycle_started.elapsed()));
            }
            observed_cycles += 1.0;
        }
    }

    pub(super) fn enable(&mut self, now: Now) {
        let result = self.enable_checked();
        match result {
            Ok(fresh) => {
                self.state.power_up(&fresh.positions, fresh.gripper_angle);
                self.state.arm().enabled(&fresh.positions, now);
            }
            Err(reason) => {
                // INVARIANT: A refused preflight leaves the arm disabled; release what the
                // attempt may have left on.
                let detail = format!("Enable failed: {reason:#}");
                match self.plant.release() {
                    Ok(()) => self.state.arm().failed(&detail, now),
                    Err(failure) => self.state.arm().enter_fault(
                        &detail,
                        &[format!("Could not confirm torque release: {failure:#}")],
                        now,
                    ),
                }
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

    // SAFETY: On shutdown, with nothing left to carry out: return to where the arm
    // rested unpowered, then release.
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
        let motion = self
            .config
            .recovery_limits()
            .clamp(&poses.rest)
            .map_err(anyhow::Error::from)
            .and_then(|home_target| self.state.arm().shutdown_return_plan(home_target))
            .and_then(|plan| self.state.start_motion(plan));
        match motion {
            Ok(()) => self.state.arm().begin_moving(),
            Err(_) => self.release(now),
        }
        false
    }
}

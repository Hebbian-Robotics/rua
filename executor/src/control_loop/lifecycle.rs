use super::runner::Runner;
use crate::arm::{Job, Now};
use crate::limits::max_joint_deviation;
use crate::plant::{Feedback, Plant};
use hebbian_robot_geometry::joint_limits::RecoveryRefusal;
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
        Ok(fresh)
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

    // WHY: Enabling may take longer than a cycle; restart the cycle timer after a long admission.
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
            .state
            .arm()
            .shutdown_return_plan()
            .and_then(|plan| self.state.start_motion(plan));
        match motion {
            Ok(()) => self.state.arm().begin_moving(),
            Err(_) => self.release(now),
        }
        false
    }
}

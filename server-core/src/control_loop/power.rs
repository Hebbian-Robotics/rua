use super::runner::Runner;
use crate::arm::Now;
use crate::config::ControllerConfig;
use crate::limits::{Trip, check_effort, max_joint_deviation};
use crate::motion_values::MeasuredPositions;
use crate::plant::{Feedback, Plant};
use hebbian_robot_geometry::joint_limits::RecoveryRefusal;
const STARTUP_HOLD_SECONDS: f64 = 0.25;
const SHUTDOWN_AT_REST_RADIANS: f64 = 0.01;

enum EnableDecision {
    ObserveAgain,
    Confirmed,
}

#[derive(Debug)]
enum EnableRefusal<'a> {
    Drift,
    FeedbackFault(&'a str),
    Effort(Trip),
}

impl std::fmt::Display for EnableRefusal<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Drift => formatter.write_str("Moved during enable"),
            Self::FeedbackFault(fault) => write!(formatter, "Enable not confirmed: {fault}"),
            Self::Effort(trip) => formatter.write_str(trip.detail()),
        }
    }
}

pub(super) struct StartupHold {
    initial: Vec<f64>,
    remaining_observations: f64,
}

impl StartupHold {
    fn new(config: &ControllerConfig, initial: MeasuredPositions<'_>) -> Self {
        Self {
            initial: initial.as_slice().to_vec(),
            remaining_observations: (STARTUP_HOLD_SECONDS * config.frequency_hertz).ceil(),
        }
    }

    fn observe<'feedback>(
        &mut self,
        config: &ControllerConfig,
        fresh: MeasuredPositions<'_>,
        fault: Option<&'feedback str>,
        efforts: &[f64],
    ) -> Result<EnableDecision, EnableRefusal<'feedback>> {
        if let Some(checks) = &config.enable_checks
            && max_joint_deviation(fresh.as_slice(), &self.initial)
                > checks.enable_drift_limit_radians
        {
            return Err(EnableRefusal::Drift);
        }
        if let Some(fault) = fault {
            return Err(EnableRefusal::FeedbackFault(fault));
        }
        check_effort(config, efforts).map_err(EnableRefusal::Effort)?;
        if config.enable_checks.is_none() || self.remaining_observations <= 0.0 {
            return Ok(EnableDecision::Confirmed);
        }
        self.remaining_observations -= 1.0;
        Ok(EnableDecision::ObserveAgain)
    }
}

impl<P: Plant> Runner<'_, P> {
    fn begin_enable(&mut self) -> anyhow::Result<StartupHold> {
        let config = self.config;
        self.plant.preflight()?;
        let initial = self.plant.read_feedback()?;
        let initial_positions = initial.measured_positions(config)?;
        if let Err(error) = config.recovery_limits().clamp(initial_positions.as_slice()) {
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
                max_joint_deviation(settled_positions.as_slice(), initial_positions.as_slice())
                    <= checks.preflight_drift_limit_radians,
                "Moved during preflight"
            );
        }
        self.plant.enable()?;
        Ok(StartupHold::new(config, initial_positions))
    }

    pub(super) fn start_enable(&mut self, now: Now) {
        match self.begin_enable() {
            Ok(hold) => self.startup_hold = Some(hold),
            Err(failure) => self.fail_enable(&format!("{failure:#}"), now),
        }
    }

    fn observe_startup(
        &mut self,
        hold: &mut StartupHold,
    ) -> anyhow::Result<(EnableDecision, Feedback)> {
        let fresh = self.plant.read_feedback()?;
        let decision = hold
            .observe(
                self.config,
                fresh.measured_positions(self.config)?,
                fresh.fault.as_deref(),
                &fresh.efforts,
            )
            .map_err(|refusal| anyhow::anyhow!("{refusal}"))?;
        self.plant.heartbeat()?;
        Ok((decision, fresh))
    }

    pub(super) fn progress_enable(
        &mut self,
        mut hold: StartupHold,
        feedback: &mut Feedback,
        now: Now,
    ) {
        if self.state.core.shutdown_requested() {
            self.fail_enable("Shutdown requested before startup hold was confirmed", now);
            return;
        }
        let (decision, fresh) = match self.observe_startup(&mut hold) {
            Ok(observation) => observation,
            Err(failure) => {
                self.fail_enable(&format!("{failure:#}"), now);
                return;
            }
        };
        self.observe_feedback(&fresh, now);
        match decision {
            EnableDecision::ObserveAgain => self.startup_hold = Some(hold),
            EnableDecision::Confirmed => {
                self.state.power_up(&fresh.positions, fresh.gripper_angle);
                self.state.arm().enabled(&fresh.positions, now);
            }
        }
        *feedback = fresh;
    }

    pub(super) fn fail_enable(&mut self, reason: &str, now: Now) {
        self.startup_hold = None;
        // INVARIANT: A refused preflight leaves the arm disabled; release what the
        // attempt may have left on.
        let detail = format!("Enable failed: {reason}");
        match self.plant.release() {
            Ok(()) => self.state.arm().failed(&detail, now),
            Err(failure) => self.state.arm().enter_fault(
                &detail,
                &[format!("Could not confirm torque release: {failure:#}")],
                now,
            ),
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
        if !self.state.arm().shutdown_requested() || !self.state.powered() {
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

#[cfg(test)]
mod tests {
    use super::*;
    use hebbian_contracts::EnableChecks;

    #[test]
    fn startup_hold_waits_a_quarter_second_rounded_up_to_a_control_period() {
        let mut config = ControllerConfig::parse(crate::config::TEST_CONFIG)
            .unwrap()
            .into_input();
        config.frequency_hertz = 102.0;
        config.enable_checks = Some(EnableChecks {
            preflight_drift_limit_radians: 0.02,
            enable_drift_limit_radians: 0.05,
        });
        let config = ControllerConfig::try_from(config).unwrap();
        let initial = MeasuredPositions::new(&[0.0, 0.0], 2).unwrap();
        let drifted = MeasuredPositions::new(&[0.06, 0.0], 2).unwrap();
        let healthy_efforts = [0.0, 0.0];
        let mut hold = StartupHold::new(&config, initial);

        for period in 0..26 {
            assert!(
                matches!(
                    hold.observe(&config, initial, None, &healthy_efforts),
                    Ok(EnableDecision::ObserveAgain)
                ),
                "confirmed before period {period} elapsed"
            );
        }
        assert!(matches!(
            hold.observe(&config, drifted, None, &healthy_efforts),
            Err(EnableRefusal::Drift)
        ));
        assert!(matches!(
            hold.observe(&config, initial, Some("voltage fault"), &healthy_efforts),
            Err(EnableRefusal::FeedbackFault("voltage fault"))
        ));
        assert!(matches!(
            hold.observe(&config, initial, None, &[11.0, 0.0]),
            Err(EnableRefusal::Effort(_))
        ));
        assert!(matches!(
            hold.observe(&config, initial, None, &healthy_efforts),
            Ok(EnableDecision::Confirmed)
        ));
    }

    #[test]
    fn startup_without_drift_checks_confirms_the_first_healthy_observation() {
        let config = ControllerConfig::parse(crate::config::TEST_CONFIG).unwrap();
        let initial = MeasuredPositions::new(&[0.0, 0.0], 2).unwrap();
        let mut hold = StartupHold::new(&config, initial);
        assert!(matches!(
            hold.observe(&config, initial, None, &[0.0, 0.0]),
            Ok(EnableDecision::Confirmed)
        ));
    }
}

#[derive(Debug)]
pub struct CommunicationFailure(pub String);

impl std::fmt::Display for CommunicationFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for CommunicationFailure {}

pub use hebbian_so101::telemetry::EnvironmentConfirmation;

#[derive(Clone, Default)]
pub struct Feedback {
    pub positions: Vec<f64>,
    pub velocities: Vec<f64>,
    pub efforts: Vec<f64>,
    // CONSTRAINT: Telemetry follows driven-motor order, with the gripper last; plants without thermometers leave it empty.
    pub temperatures_celsius: Vec<f64>,
    pub voltages_volts: Vec<f64>,
    // INVARIANT: Follow-up confirmations do not replace the initial telemetry arrays.
    pub environment_confirmations: Vec<EnvironmentConfirmation>,
    pub gripper_angle: Option<f64>,
    pub fault: Option<String>,
}

impl Feedback {
    pub(crate) fn measured_positions(
        &self,
        config: &crate::config::ControllerConfig,
    ) -> anyhow::Result<crate::motion_values::MeasuredPositions<'_>> {
        anyhow::ensure!(
            self.gripper_angle.is_none_or(f64::is_finite),
            "Invalid gripper feedback"
        );
        anyhow::ensure!(
            config.gripper.is_none() || self.gripper_angle.is_some(),
            "Missing feedback for the configured gripper"
        );
        crate::motion_values::MeasuredPositions::new(&self.positions, config.joint_count)
    }
}

pub struct RecoveryObservation {
    pub feedback: Feedback,
    pub motor_torque: Vec<(u8, bool)>,
    pub health_issue: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pacing {
    Clock,
    // CONSTRAINT: Reply-paced simulation advances by the nominal period and waits out wall time so station timeouts remain meaningful.
    RepliesAtClockSpeed,
    RepliesAsFastAsPossible,
}

impl Pacing {
    #[must_use]
    pub(crate) fn is_set_by_replies(self) -> bool {
        self != Self::Clock
    }
}

pub enum Reconnection {
    Waiting,
    Failed(anyhow::Error),
    Restored,
}

// SAFETY: A plant that cannot be read or commanded leaves the loop unable to keep the
// arm under control. An enabled arm is released first as well as the plant
// allows; one that cannot be released keeps holding its last goal. A simulator
// that is gone ends the loop; a bus can reconnect and verify its hold.
pub trait Plant {
    fn observe_recovery(&mut self) -> anyhow::Result<RecoveryObservation> {
        anyhow::bail!("Recovery inspection is not supported by this plant")
    }

    fn hold_recovery(&mut self) -> anyhow::Result<RecoveryObservation> {
        anyhow::bail!("Energized recovery is not supported by this plant")
    }
    // SAFETY: Recovery may re-enable lost torque only after the operator supports the arm and configuration, health and stability checks pass.
    fn enable_recovery(
        &mut self,
        _drift_limit_radians: f64,
    ) -> anyhow::Result<RecoveryObservation> {
        anyhow::bail!("Approved recovery is not supported by this plant")
    }
    // CONSTRAINT: Replace every feedback field and reuse its buffers to avoid allocations in the control cycle. Failed reads leave feedback unspecified.
    fn read_feedback_into(&mut self, feedback: &mut Feedback) -> anyhow::Result<()>;

    fn read_feedback(&mut self) -> anyhow::Result<Feedback> {
        let mut feedback = Feedback::default();
        self.read_feedback_into(&mut feedback)?;
        Ok(feedback)
    }
    fn command(&mut self, positions: &[f64], seconds: f64) -> anyhow::Result<()>;
    fn command_gripper(&mut self, angle: f64) -> anyhow::Result<()>;
    fn preflight(&mut self) -> anyhow::Result<()>;
    fn enable(&mut self) -> anyhow::Result<()>;
    fn release(&mut self) -> anyhow::Result<()>;

    // SAFETY: Poll getting a plant back after it stopped answering, without blocking the
    // cycle. Torque is never turned on by it. A plant with no connection to
    // reopen is back once it can be read.
    fn reconnect(&mut self) -> Reconnection {
        match self.read_feedback() {
            Ok(_) => Reconnection::Restored,
            Err(failure) => Reconnection::Failed(failure),
        }
    }

    fn pacing(&self) -> Pacing {
        Pacing::Clock
    }

    fn advance(&mut self, _seconds: f64) -> anyhow::Result<()> {
        Ok(())
    }

    // SAFETY: Start the guard before enabling torque so it can take over if this process stops.
    fn start_guard(&mut self) -> anyhow::Result<()> {
        Ok(())
    }

    // SAFETY: An enabled arm requires a heartbeat every cycle; losing the guard leaves it unwatched.
    fn heartbeat(&mut self) -> anyhow::Result<()> {
        Ok(())
    }

    // SAFETY: Stop the guard only after the motors are off.
    fn stop_guard(&mut self) -> Vec<String> {
        Vec::new()
    }
}

// WHY: Stands in for an arm where physics does not matter: tests, the viewer's reach preview and
// `serve --no-physics-stand-in`. Joints ease toward their commands with a time constant; nothing
// models contacts, the table, gravity, loads or the real servos' delay, so it cannot judge motion.
pub struct NoPhysicsStandIn {
    positions: Vec<f64>,
    gripper_angle: Option<f64>,
    velocities: Vec<f64>,
    time_constant_seconds: f64,
}

impl NoPhysicsStandIn {
    #[must_use]
    pub fn at(positions: Vec<f64>, time_constant_seconds: f64) -> Self {
        let velocities = vec![0.0; positions.len()];
        Self {
            positions,
            gripper_angle: None,
            velocities,
            time_constant_seconds,
        }
    }

    #[must_use]
    pub fn with_gripper(mut self, angle: f64) -> Self {
        self.gripper_angle = Some(angle);
        self
    }
}

impl Plant for NoPhysicsStandIn {
    // CONSTRAINT: Physics advances by the configured timestep, as with the socket simulator.
    // OS scheduling jitter is not physical servo lag and must not alter the plant.
    fn pacing(&self) -> Pacing {
        Pacing::RepliesAtClockSpeed
    }

    fn read_feedback_into(&mut self, feedback: &mut Feedback) -> anyhow::Result<()> {
        feedback.positions.clone_from(&self.positions);
        feedback.velocities.clone_from(&self.velocities);
        feedback.efforts.clear();
        feedback.efforts.resize(self.positions.len(), 0.0);
        feedback.temperatures_celsius.clear();
        feedback.voltages_volts.clear();
        feedback.environment_confirmations.clear();
        feedback.gripper_angle = self.gripper_angle;
        feedback.fault = None;
        Ok(())
    }

    fn command(&mut self, positions: &[f64], seconds: f64) -> anyhow::Result<()> {
        let blend = 1.0 - (-seconds / self.time_constant_seconds).exp();
        for (index, target) in positions.iter().enumerate() {
            let step = (target - self.positions[index]) * blend;
            self.positions[index] += step;
            self.velocities[index] = step / seconds;
        }
        Ok(())
    }

    fn command_gripper(&mut self, angle: f64) -> anyhow::Result<()> {
        anyhow::ensure!(self.gripper_angle.is_some(), "This arm has no gripper");
        self.gripper_angle = Some(angle);
        Ok(())
    }

    fn preflight(&mut self) -> anyhow::Result<()> {
        Ok(())
    }

    fn enable(&mut self) -> anyhow::Result<()> {
        Ok(())
    }

    fn release(&mut self) -> anyhow::Result<()> {
        Ok(())
    }
}

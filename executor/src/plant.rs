//! What the loop commands and reads back: a real bus, or a model of one.

/// A plant could not exchange a command or reading with its hardware.
#[derive(Debug)]
pub struct CommunicationFailure(pub String);

impl std::fmt::Display for CommunicationFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for CommunicationFailure {}

// Independent environmental confirmation is a hardware observation.
pub use hebbian_so101::telemetry::EnvironmentConfirmation;

#[derive(Clone, Default)]
pub struct Feedback {
    pub positions: Vec<f64>,
    pub velocities: Vec<f64>,
    pub efforts: Vec<f64>,
    /// The last temperature reading of each driven motor, the gripper's last;
    /// empty for a plant without thermometers.
    pub temperatures_celsius: Vec<f64>,
    /// Supply voltage for each driven motor in the same order as temperatures.
    pub voltages_volts: Vec<f64>,
    /// Follow-up readings; initial values remain in the regular arrays.
    pub environment_confirmations: Vec<EnvironmentConfirmation>,
    /// The jaw's angle in the URDF's gripper joint, when the arm has a gripper.
    pub gripper_angle: Option<f64>,
    /// A fault the hardware reports (an alarm, overheating): the loop trips on it.
    pub fault: Option<String>,
}

/// A fresh read after a communication interruption, before issuing a new goal.
pub struct RecoveryObservation {
    pub feedback: Feedback,
    /// Motor IDs and their observed torque-enable bits.
    pub motor_torque: Vec<(u8, bool)>,
    pub health_issue: Option<String>,
}

/// What sets the loop's pace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pacing {
    /// The clock: each cycle begins at its absolute deadline. Real hardware.
    Clock,
    /// The plant's replies: each cycle lasts the nominal period whatever the wall
    /// time. A simulator over a socket is paced this way, and its cycles also
    /// wait out the wall-clock period so simulated time runs at the clock's speed.
    RepliesAtClockSpeed,
    /// As `RepliesAtClockSpeed`, but cycles run as fast as the replies come.
    RepliesAsFastAsPossible,
}

impl Pacing {
    #[must_use]
    pub(crate) fn is_set_by_replies(self) -> bool {
        self != Self::Clock
    }
}

/// How getting a lost plant back stands.
pub enum Reconnection {
    /// No attempt finished since the last poll.
    Waiting,
    /// An attempt failed; another follows.
    Failed(anyhow::Error),
    /// The plant answers again, set up as at startup, with every motor heard.
    Restored,
}

/// A plant that cannot be read or commanded leaves the loop unable to keep the
/// arm under control. An enabled arm is released first as well as the plant
/// allows; one that cannot be released keeps holding its last goal. A simulator
/// that is gone ends the loop; a bus can reconnect and verify its hold.
pub trait Plant {
    /// # Errors
    /// The plant does not support recovery inspection or a required read failed.
    fn observe_recovery(&mut self) -> anyhow::Result<RecoveryObservation> {
        anyhow::bail!("Recovery inspection is not supported by this plant")
    }

    /// # Errors
    /// The plant cannot verify an energized hold at the measured position.
    fn hold_recovery(&mut self) -> anyhow::Result<RecoveryObservation> {
        anyhow::bail!("Energized recovery is not supported by this plant")
    }
    /// Re-enable motors that lost torque after the operator has supported the arm.
    ///
    /// # Errors
    /// Configuration, health, position stability, or torque could not be verified.
    fn enable_recovery(
        &mut self,
        _drift_limit_radians: f64,
    ) -> anyhow::Result<RecoveryObservation> {
        anyhow::bail!("Approved recovery is not supported by this plant")
    }
    /// Read the plant into `feedback`, replacing every field and reusing its
    /// buffers, so a cycle's reading allocates nothing once they have grown.
    ///
    /// # Errors
    /// The plant did not answer; `feedback` is then unspecified.
    fn read_feedback_into(&mut self, feedback: &mut Feedback) -> anyhow::Result<()>;

    /// A reading in fresh buffers, for what is not on the cycle's path.
    ///
    /// # Errors
    /// The plant did not answer.
    fn read_feedback(&mut self) -> anyhow::Result<Feedback> {
        let mut feedback = Feedback::default();
        self.read_feedback_into(&mut feedback)?;
        Ok(feedback)
    }
    /// # Errors
    /// The plant did not take the command.
    fn command(&mut self, positions: &[f64], seconds: f64) -> anyhow::Result<()>;
    /// Send the gripper toward this angle (the URDF gripper joint's).
    ///
    /// # Errors
    /// The plant has no gripper, or did not take the command.
    fn command_gripper(&mut self, angle: f64) -> anyhow::Result<()>;
    /// Check the hardware against the validated configuration; reads only.
    ///
    /// # Errors
    /// The hardware does not match, or did not answer.
    fn preflight(&mut self) -> anyhow::Result<()>;
    /// Turn torque on holding the present pose.
    ///
    /// # Errors
    /// The plant did not take it.
    fn enable(&mut self) -> anyhow::Result<()>;
    /// Turn torque off, which drops the arm.
    ///
    /// # Errors
    /// The plant did not take it.
    fn release(&mut self) -> anyhow::Result<()>;

    /// Poll getting a plant back after it stopped answering, without blocking the
    /// cycle. Torque is never turned on by it. A plant with no connection to
    /// reopen is back once it can be read.
    fn reconnect(&mut self) -> Reconnection {
        match self.read_feedback() {
            Ok(_) => Reconnection::Restored,
            Err(failure) => Reconnection::Failed(failure),
        }
    }

    /// What sets the loop's pace.
    fn pacing(&self) -> Pacing {
        Pacing::Clock
    }

    /// Let a paced plant move time on by `seconds` in a cycle where nothing was
    /// commanded (the arm has no torque). Real plants have time moving on their own.
    ///
    /// # Errors
    /// The plant did not take it.
    fn advance(&mut self, _seconds: f64) -> anyhow::Result<()> {
        Ok(())
    }

    /// Start the safety guard that watches the arm if this process stops, before
    /// torque goes on. Plants without one do nothing.
    ///
    /// # Errors
    /// The guard did not start.
    fn start_guard(&mut self) -> anyhow::Result<()> {
        Ok(())
    }

    /// Tell the guard this process is alive; every cycle while enabled.
    ///
    /// # Errors
    /// The guard is gone, which leaves the arm unwatched.
    fn heartbeat(&mut self) -> anyhow::Result<()> {
        Ok(())
    }

    /// Let the guard go once the motors are off; what went wrong doing so.
    fn stop_guard(&mut self) -> Vec<String> {
        Vec::new()
    }
}

/// Joints that follow their command through a first-order lag. It stands in for
/// servos so the loop's timing can be measured without hardware.
pub struct SimulatedPlant {
    positions: Vec<f64>,
    gripper_angle: Option<f64>,
    velocities: Vec<f64>,
    time_constant_seconds: f64,
}

impl SimulatedPlant {
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

    /// Give the arm a jaw that sits at `angle` and moves there at once.
    #[must_use]
    pub fn with_gripper(mut self, angle: f64) -> Self {
        self.gripper_angle = Some(angle);
        self
    }
}

impl Plant for SimulatedPlant {
    // Physics advances by the configured timestep, as with the socket simulator.
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

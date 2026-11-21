#![allow(
    clippy::disallowed_types,
    clippy::disallowed_methods,
    reason = "This adapter translates control-loop operations to the standalone hardware driver."
)]

use crate::plant::{CommunicationFailure, Feedback, Plant, Reconnection, RecoveryObservation};
pub use hebbian_so101::driver::{BusTiming, SerialBus, ServoBus, Servos, StateBlock};
use hebbian_so101::telemetry as hardware;

fn station_error(error: anyhow::Error) -> anyhow::Error {
    if error.is::<hardware::CommunicationFailure>() {
        let message = error.to_string();
        error.context(CommunicationFailure(message))
    } else {
        error
    }
}

impl From<hardware::Feedback> for Feedback {
    fn from(feedback: hardware::Feedback) -> Self {
        Self {
            positions: feedback.positions,
            velocities: feedback.velocities,
            efforts: feedback.efforts,
            temperatures_celsius: feedback.temperatures_celsius,
            voltages_volts: feedback.voltages_volts,
            environment_confirmations: feedback.environment_confirmations,
            gripper_angle: feedback.gripper_angle,
            fault: feedback.fault,
        }
    }
}

impl From<Feedback> for hardware::Feedback {
    fn from(feedback: Feedback) -> Self {
        Self {
            positions: feedback.positions,
            velocities: feedback.velocities,
            efforts: feedback.efforts,
            temperatures_celsius: feedback.temperatures_celsius,
            voltages_volts: feedback.voltages_volts,
            environment_confirmations: feedback.environment_confirmations,
            gripper_angle: feedback.gripper_angle,
            fault: feedback.fault,
        }
    }
}

impl From<hardware::RecoveryObservation> for RecoveryObservation {
    fn from(observation: hardware::RecoveryObservation) -> Self {
        Self {
            feedback: observation.feedback.into(),
            motor_torque: observation.motor_torque,
            health_issue: observation.health_issue,
        }
    }
}

impl<B: ServoBus> Plant for Servos<B> {
    fn read_feedback_into(&mut self, feedback: &mut Feedback) -> anyhow::Result<()> {
        // INVARIANT: Move buffers across the boundary and return them even when a read fails.
        // No telemetry cloning or additional per-cycle allocation is introduced.
        let mut hardware_feedback = std::mem::take(feedback).into();
        let result =
            Servos::read_feedback_into(self, &mut hardware_feedback).map_err(station_error);
        *feedback = hardware_feedback.into();
        result
    }

    fn observe_recovery(&mut self) -> anyhow::Result<RecoveryObservation> {
        Servos::observe_recovery(self)
            .map(Into::into)
            .map_err(station_error)
    }

    fn hold_recovery(&mut self) -> anyhow::Result<RecoveryObservation> {
        Servos::hold_recovery(self)
            .map(Into::into)
            .map_err(station_error)
    }

    fn enable_recovery(&mut self, drift_limit_radians: f64) -> anyhow::Result<RecoveryObservation> {
        Servos::enable_recovery(self, drift_limit_radians)
            .map(Into::into)
            .map_err(station_error)
    }

    fn command(&mut self, positions: &[f64], _seconds: f64) -> anyhow::Result<()> {
        Servos::command(self, positions).map_err(station_error)
    }

    fn command_gripper(&mut self, angle: f64) -> anyhow::Result<()> {
        Servos::command_gripper(self, angle).map_err(station_error)
    }

    fn preflight(&mut self) -> anyhow::Result<()> {
        Servos::preflight(self).map_err(station_error)
    }

    fn enable(&mut self) -> anyhow::Result<()> {
        Servos::enable(self).map_err(station_error)
    }

    fn release(&mut self) -> anyhow::Result<()> {
        Servos::release(self).map_err(station_error)
    }

    fn reconnect(&mut self) -> Reconnection {
        match Servos::reconnect(self) {
            hardware::Reconnection::Waiting => Reconnection::Waiting,
            hardware::Reconnection::Restored => Reconnection::Restored,
            hardware::Reconnection::Failed(error) => Reconnection::Failed(station_error(error)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hardware_observations_reach_the_station_without_losing_health_evidence() {
        let observation = hardware::RecoveryObservation {
            feedback: hardware::Feedback {
                positions: vec![0.25],
                velocities: vec![0.1],
                efforts: vec![12.0],
                temperatures_celsius: vec![40.0, 42.0],
                voltages_volts: vec![12.0, 11.9],
                environment_confirmations: vec![hardware::EnvironmentConfirmation {
                    motor_id: 6,
                    temperature_celsius: 70.0,
                    voltage_volts: 10.0,
                    status: 4,
                    packet_status: 1,
                }],
                gripper_angle: Some(0.75),
                fault: Some("temperature".into()),
            },
            motor_torque: vec![(1, true), (6, false)],
            health_issue: Some("Motor torque changed".into()),
        };
        let station: RecoveryObservation = observation.into();
        assert_eq!(station.motor_torque, [(1, true), (6, false)]);
        assert_eq!(
            station.health_issue.as_deref(),
            Some("Motor torque changed")
        );
        let feedback = station.feedback;
        assert_eq!(feedback.positions, [0.25]);
        assert_eq!(feedback.velocities, [0.1]);
        assert_eq!(feedback.efforts, [12.0]);
        assert_eq!(feedback.temperatures_celsius, [40.0, 42.0]);
        assert_eq!(feedback.voltages_volts, [12.0, 11.9]);
        assert_eq!(feedback.gripper_angle, Some(0.75));
        assert_eq!(feedback.fault.as_deref(), Some("temperature"));
        let confirmation = &feedback.environment_confirmations[0];
        assert_eq!(
            (
                confirmation.motor_id,
                confirmation.status,
                confirmation.packet_status
            ),
            (6, 4, 1)
        );
        assert_eq!(
            (confirmation.temperature_celsius, confirmation.voltage_volts),
            (70.0, 10.0)
        );
    }

    #[test]
    fn bus_failures_remain_reconnectable_and_configuration_errors_do_not() {
        let failure = anyhow::Error::new(hardware::CommunicationFailure("no reply".into()))
            .context("Reading joint state");
        let failure = station_error(failure);
        assert!(failure.is::<CommunicationFailure>());
        assert!(failure.is::<hardware::CommunicationFailure>());
        assert!(format!("{failure:#}").contains("no reply"));
        let configuration_error = station_error(anyhow::anyhow!("Gains differ"));
        assert!(!configuration_error.is::<CommunicationFailure>());
    }
}

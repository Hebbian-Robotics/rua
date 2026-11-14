//! Adapt SO-101 calibration and settings to the executor's existing wire config.

use std::path::Path;

use anyhow::Context;

use super::calibration::{LerobotMotorCalibration, load_calibration};
use super::settings::{FeetechSettings, Sign};
use crate::config::{FeetechConfig, FeetechGripper, FeetechMotor, Gains, MotorCalibration};
use crate::profile::GripperSettings;

fn feetech_motor(
    settings: &FeetechSettings,
    calibration: &std::collections::BTreeMap<u8, LerobotMotorCalibration>,
    motor_id: u8,
    sign: Sign,
    offset_radians: f64,
) -> FeetechMotor {
    let entry = &calibration[&motor_id];
    let [p, i, d] = settings.expected_gains(motor_id);
    FeetechMotor {
        id: motor_id,
        sign: sign.as_integer(),
        offset_radians,
        calibration: MotorCalibration {
            homing_offset: entry.homing_offset,
            range_min: entry.range_min,
            range_max: entry.range_max,
        },
        expected_gains: Gains { p, i, d },
    }
}

pub(crate) fn feetech_bus(
    gripper_settings: Option<&GripperSettings>,
    settings: &FeetechSettings,
    arm: &str,
    calibration_root: &Path,
) -> anyhow::Result<FeetechConfig> {
    let calibration = load_calibration(settings, arm, calibration_root)?;
    let joints = settings
        .motor_ids
        .iter()
        .zip(&settings.joint_signs)
        .zip(&settings.joint_offsets_radians)
        .map(|((motor_id, sign), offset)| {
            feetech_motor(settings, &calibration, *motor_id, *sign, *offset)
        })
        .collect();
    let gripper = match (settings.gripper_motor_id, gripper_settings) {
        (Some(motor_id), Some(gripper)) => Some(FeetechGripper {
            motor: feetech_motor(
                settings,
                &calibration,
                motor_id,
                settings.gripper_sign,
                settings.gripper_offset_radians,
            ),
            force_fraction: gripper.force_fraction,
        }),
        _ => None,
    };
    Ok(FeetechConfig {
        port: settings
            .ports
            .get(arm)
            .with_context(|| format!("No port for arm {arm}"))?
            .clone(),
        joints,
        gripper,
        passive_motor_ids: settings.passive_motor_ids.clone(),
        expected_return_delay_time: settings.expected_return_delay_time.map(i64::from),
        max_temperature_celsius: settings.max_temperature_celsius,
        min_voltage_volts: settings.min_voltage_volts,
        max_voltage_volts: settings.max_voltage_volts,
    })
}

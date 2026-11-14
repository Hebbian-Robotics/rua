//! The station as clients see it, built from each arm's last report. An arm
//! whose reports have stopped is never shown as live: it reads as faulted,
//! with the reason.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::control_link::LinkHandle;
use hebbian_contracts::{ArmMode, ArmState, ControllerKind, StationState};

/// An arm sends a sample every cycle, enabled or not; this long without one,
/// its state is not live.
const STALE_AFTER: Duration = Duration::from_secs(1);

async fn arm_state(arm: &str, link: &LinkHandle) -> ArmState {
    let config = link.config();
    let view = link.view().await;
    let sample = view.as_ref().and_then(|view| view.sample.clone());
    let age = view.as_ref().and_then(|view| view.sample_age);
    let controller = if link.has_policy() {
        ControllerKind::Policy
    } else {
        ControllerKind::Ik
    };
    let stale = match age {
        None => Some("No feedback from the arm yet".to_owned()),
        Some(age) if age > STALE_AFTER => Some(format!(
            "No feedback for {:.1} s; the arm's state is not live",
            age.as_secs_f64()
        )),
        Some(_) => None,
    };
    let mode = match (&stale, &sample) {
        (Some(_), _) | (None, None) => ArmMode::Faulted,
        (None, Some(sample)) => sample.mode,
    };
    let status = stale.clone().unwrap_or_else(|| {
        sample.as_ref().map_or_else(String::new, |sample| {
            sample
                .fault
                .clone()
                .unwrap_or_else(|| format!("{:?}", sample.mode).to_lowercase())
        })
    });
    let tool = sample.as_ref().and_then(|sample| {
        link.planner()
            .and_then(|planner| planner.describe(&sample.positions).ok())
    });
    let sampled_at_unix_seconds = age.and_then(|age| {
        SystemTime::now()
            .checked_sub(age)
            .and_then(|at| at.duration_since(UNIX_EPOCH).ok())
            .map(|since| since.as_secs_f64())
    });
    let (interlocks, revision) = link.interlocks();
    ArmState {
        arm: arm.to_owned(),
        interlocks,
        revision,
        mode,
        sampled_at_unix_seconds,
        measured_joints: sample
            .as_ref()
            .map(|sample| sample.positions.clone())
            .unwrap_or_default(),
        commanded_joints: sample
            .as_ref()
            .and_then(|sample| sample.commanded_positions.clone()),
        tool,
        controller,
        fault: stale.or_else(|| sample.as_ref().and_then(|sample| sample.fault.clone())),
        recovery_issue: sample
            .as_ref()
            .and_then(|sample| sample.recovery_issue.clone()),
        status,
        rest_pose: sample.as_ref().and_then(|sample| sample.rest_pose.clone()),
        segment: sample.as_ref().and_then(|sample| sample.segment.clone()),
        active_command: view.as_ref().and_then(|view| view.active_record.clone()),
        last_command: view.as_ref().and_then(|view| view.last_record.clone()),
        gripper_opening: match (
            &config.gripper,
            sample.as_ref().and_then(|s| s.gripper_angle),
        ) {
            (Some(gripper), Some(angle)) => Some(gripper.opening(angle)),
            _ => None,
        },
    }
}

/// Every arm's state and what clients are told about the robot.
pub async fn station_state(links: &BTreeMap<String, LinkHandle>) -> Option<StationState> {
    let first = links.values().next()?;
    let mut arms = Vec::new();
    for (arm, link) in links {
        arms.push(arm_state(arm, link).await);
    }
    let config = first.config();
    let station = config.station.clone();
    Some(StationState {
        robot: station.as_ref().map(|station| station.robot.clone()),
        description: station.as_ref().map(|station| station.description.clone()),
        joint_names: station.map_or_else(
            || config.driven_joints[..config.joint_count].to_vec(),
            |station| station.joint_names,
        ),
        surface_guard: config.surface_guard.clone(),
        jaw_closing_axis: config.jaw_closing_axis,
        max_joint_speed: config.max_joint_speed,
        arms,
        lease: first.lease_status(),
    })
}

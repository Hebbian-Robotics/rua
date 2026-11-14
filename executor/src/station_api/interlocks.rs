//! Raise and clear each arm's own interlocks from its reported mode: the bus
//! stopping (`arm_disconnected`), and a reconnection that needs an operator's
//! approval (`recovery_approval_required`). A required camera's interlock
//! belongs to the lease (see [`crate::lease`]).

use std::time::Duration;

use hebbian_contracts::{ArmMode, Interlock};

use crate::control_link::LinkHandle;

/// A sample older than this says nothing about the arm now.
const FRESH_SAMPLE: Duration = Duration::from_secs(1);

/// Follow one arm's mode for as long as its link lives.
pub async fn watch_arm(link: LinkHandle) {
    let mut changes = link.changes();
    loop {
        update(&link).await;
        if changes.changed().await.is_err() {
            return;
        }
    }
}

async fn update(link: &LinkHandle) {
    let Ok(Some(view)) = tokio::time::timeout(FRESH_SAMPLE, link.view()).await else {
        return;
    };
    if view.sample_age.is_none_or(|age| age > FRESH_SAMPLE) {
        return;
    }
    let Some(sample) = view.sample else { return };
    match sample.mode {
        ArmMode::Disconnected => match sample.recovery_issue {
            Some(issue) => {
                link.clear_interlocks(|interlock| *interlock == Interlock::ArmDisconnected);
                link.raise_interlock(Interlock::RecoveryApprovalRequired {
                    motors: issue
                        .motors_without_torque
                        .iter()
                        .map(ToString::to_string)
                        .collect(),
                });
            }
            None => link.raise_interlock(Interlock::ArmDisconnected),
        },
        // Holding again, or powered off on purpose: nothing left to approve.
        ArmMode::Holding | ArmMode::Disabled | ArmMode::Faulted => {
            link.clear_interlocks(|interlock| {
                matches!(
                    interlock,
                    Interlock::ArmDisconnected | Interlock::RecoveryApprovalRequired { .. }
                )
            });
        }
        ArmMode::Moving | ArmMode::Reconnected => {}
    }
}

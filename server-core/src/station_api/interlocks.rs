use std::time::Duration;

use hebbian_contracts::{ArmMode, Interlock};

use crate::control_link::LinkHandle;

const FRESH_SAMPLE: Duration = Duration::from_secs(1);

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
        ArmMode::Disconnected => {
            if let Some(issue) = sample.recovery_issue {
                link.raise_interlock(Interlock::RecoveryApprovalRequired {
                    motors: issue
                        .motors_without_torque
                        .iter()
                        .map(ToString::to_string)
                        .collect(),
                });
                link.clear_interlocks(|interlock| *interlock == Interlock::ArmDisconnected);
            } else {
                link.raise_interlock(Interlock::ArmDisconnected);
                link.clear_interlocks(|interlock| {
                    matches!(interlock, Interlock::RecoveryApprovalRequired { .. })
                });
            }
        }
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use hebbian_contracts::{ArmRecoveryIssue, Event, Sample};

    use super::*;
    use crate::config::ExecutorConfig;
    use crate::control_link::Link;
    use crate::lease::{LEASE, Lease};

    #[tokio::test]
    async fn reconnection_replaces_an_obsolete_recovery_approval_interlock() {
        let config =
            ExecutorConfig::parse(include_str!("../../tests/support/two_joint_config.json"))
                .unwrap();
        let link = Link::new(&config, Arc::new(Lease::new(LEASE))).unwrap();
        let handle = link.handle();
        let mut sample = Sample::blank();
        sample.mode = ArmMode::Disconnected;
        sample.recovery_issue = Some(ArmRecoveryIssue {
            motors_without_torque: vec![2],
            detail: "Motor lost torque".into(),
        });
        link.publish(Event::State(sample.clone()));
        update(&handle).await;
        assert_eq!(
            handle.interlocks().0,
            vec![Interlock::RecoveryApprovalRequired {
                motors: vec!["2".into()]
            }]
        );

        sample.recovery_issue = None;
        sample.cycle = 1;
        link.publish(Event::State(sample));
        update(&handle).await;
        assert_eq!(handle.interlocks().0, vec![Interlock::ArmDisconnected]);
        link.close();
    }
}

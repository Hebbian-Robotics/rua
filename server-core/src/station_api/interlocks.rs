use std::time::Duration;

use hebbian_contracts::{ArmMode, Interlock};

use crate::control_link::LinkHandle;

const FRESH_SAMPLE: Duration = Duration::from_secs(1);

pub(super) async fn watch_arm(link: LinkHandle, admission: super::admission::AdmissionOwner) {
    let mut changes = link.changes();
    loop {
        if update(&link, &admission).await.is_err() {
            return;
        }
        if changes.changed().await.is_err() {
            return;
        }
    }
}

async fn update(
    link: &LinkHandle,
    admission: &super::admission::AdmissionOwner,
) -> Result<(), super::admission::AdmissionClosed> {
    let Ok(Some(view)) = tokio::time::timeout(FRESH_SAMPLE, link.view()).await else {
        return Ok(());
    };
    if view.sample_age.is_none_or(|age| age > FRESH_SAMPLE) {
        return Ok(());
    }
    let Some(sample) = view.sample else {
        return Ok(());
    };
    let link = link.clone();
    admission.decide(move |_| apply_sample(&link, sample)).await
}

fn apply_sample(link: &LinkHandle, sample: hebbian_contracts::Sample) {
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

    use hebbian_contracts::{ArmRecoveryIssue, Sample};

    use super::*;
    use crate::config::ControllerConfig;
    use crate::control_link::Link;
    use crate::lease::{LEASE, Lease};

    #[tokio::test]
    async fn reconnection_replaces_an_obsolete_recovery_approval_interlock() {
        let config =
            ControllerConfig::parse(include_str!("../../tests/support/two_joint_config.json"))
                .unwrap();
        let link = Link::new(&config, Arc::new(Lease::new(LEASE))).unwrap();
        let handle = link.handle();
        let admission = super::super::admission::AdmissionOwner::start(
            &tokio::runtime::Handle::current(),
            Arc::new(super::super::events::EventJournal::default()),
        );
        let mut sample = Sample::blank();
        sample.mode = ArmMode::Disconnected;
        sample.recovery_issue = Some(ArmRecoveryIssue {
            motors_without_torque: vec![2],
            detail: "Motor lost torque".into(),
        });
        link.publish_sample(sample.clone());
        update(&handle, &admission).await.unwrap();
        assert_eq!(
            handle.interlocks().0,
            vec![Interlock::RecoveryApprovalRequired {
                motors: vec!["2".into()]
            }]
        );

        sample.recovery_issue = None;
        sample.cycle = 1;
        link.publish_sample(sample);
        update(&handle, &admission).await.unwrap();
        assert_eq!(handle.interlocks().0, vec![Interlock::ArmDisconnected]);
        link.close();
    }
}

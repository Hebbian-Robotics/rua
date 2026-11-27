use std::time::Duration;

use hebbian_contracts::{ArmMode, Interlock, Sample};

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

enum ConnectionInterlock {
    Keep,
    Clear,
    Hold(Interlock),
}

fn connection_interlock(sample: Sample) -> ConnectionInterlock {
    match sample.mode {
        ArmMode::Disconnected => {
            let interlock = match sample.recovery_issue {
                Some(issue) => Interlock::RecoveryApprovalRequired {
                    motors: issue
                        .motors_without_torque
                        .iter()
                        .map(ToString::to_string)
                        .collect(),
                },
                None => Interlock::ArmDisconnected,
            };
            ConnectionInterlock::Hold(interlock)
        }
        ArmMode::Holding | ArmMode::Disabled | ArmMode::Faulted | ArmMode::Reconnected => {
            ConnectionInterlock::Clear
        }
        ArmMode::Moving => ConnectionInterlock::Keep,
    }
}

fn apply_sample(link: &LinkHandle, sample: Sample) {
    let retained_kind = match connection_interlock(sample) {
        ConnectionInterlock::Keep => return,
        ConnectionInterlock::Clear => None,
        ConnectionInterlock::Hold(interlock) => {
            let kind = std::mem::discriminant(&interlock);
            // INVARIANT: Raise the replacement before clearing to keep motion interlocked.
            link.raise_interlock(interlock);
            Some(kind)
        }
    };
    link.clear_interlocks(|interlock| {
        matches!(
            interlock,
            Interlock::ArmDisconnected | Interlock::RecoveryApprovalRequired { .. }
        ) && Some(std::mem::discriminant(interlock)) != retained_kind
    });
}

// CONSTRAINT: Manual control can proceed with a required camera unavailable. Other interlocks on any arm still block it.
pub(super) fn manual_control_ready_without_waiting(links: &[LinkHandle]) -> Result<(), String> {
    for link in links {
        if let Some(interlock) = link
            .interlocks()
            .0
            .into_iter()
            .find(|interlock| !matches!(interlock, Interlock::RequiredCameraUnavailable { .. }))
        {
            return Err(format!(
                "Manual control is inhibited by an interlock: {interlock:?}"
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use hebbian_contracts::ArmRecoveryIssue;

    use super::*;
    use crate::config::ControllerConfig;
    use crate::control_link::Link;
    use crate::lease::{LEASE, Lease};

    #[tokio::test]
    async fn reconnection_reconciles_recovery_interlocks() {
        let config =
            ControllerConfig::parse(include_str!("../../tests/support/two_joint_config.json"))
                .unwrap();
        let (link, _link_endpoint) = Link::new(&config, Arc::new(Lease::new(LEASE))).unwrap();
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
        link.publish_sample(sample.clone());
        update(&handle, &admission).await.unwrap();
        assert_eq!(handle.interlocks().0, vec![Interlock::ArmDisconnected]);

        sample.mode = ArmMode::Moving;
        sample.cycle = 2;
        link.publish_sample(sample.clone());
        update(&handle, &admission).await.unwrap();
        assert_eq!(handle.interlocks().0, vec![Interlock::ArmDisconnected]);

        sample.mode = ArmMode::Reconnected;
        sample.cycle = 3;
        link.publish_sample(sample);
        update(&handle, &admission).await.unwrap();
        assert_eq!(handle.interlocks().0, [] as [Interlock; 0]);
        link.close();
    }
}

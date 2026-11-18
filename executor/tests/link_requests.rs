#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    reason = "This integration-test target may abort on failed setup or unexpected outcomes."
)]

use std::sync::Arc;

use hebbian_robot_use_server::config::ExecutorConfig;
use hebbian_robot_use_server::control_link::Link;
use hebbian_robot_use_server::lease::{LEASE, Lease};
use serde_json::json;

mod support;
use hebbian_contracts::{
    Command, Event, LeaseAnswer, LeaseHolder, OnInterruption, RejectionReason,
};
use hebbian_robot_use_server::lease::LeaseRequest;
use support::LinkClient;

const CONFIG: &str = include_str!("support/two_joint_config.json");

fn link() -> Link {
    let config = ExecutorConfig::parse(CONFIG).unwrap();
    Link::new(&config, Arc::new(Lease::new(LEASE))).unwrap()
}

fn reply(client: &mut LinkClient) -> Event {
    client.read_until(|event| matches!(event, Event::Reply { .. }))
}

#[test]
fn a_resent_request_id_is_answered_again_not_acted_on_again() {
    let link = link();
    let mut client = LinkClient::connect(&link.handle());
    let claim = json!({"kind": "acquire_lease", "holder": "agent", "label": "x", "required_cameras": [], "on_interruption": "wait_for_approval"});

    client.send_request(Some("a"), &claim);
    let first = reply(&mut client);
    assert!(matches!(
        &first,
        Event::Reply {
            accepted: true,
            lease: Some(answer),
            ..
        } if matches!(**answer, LeaseAnswer::Granted(_))
    ));
    // WHY: Acted on again, the claim would be refused: control is already held.
    client.send_request(Some("a"), &claim);
    assert_eq!(reply(&mut client), first);
    link.close();
}

#[test]
fn unreadable_requests_are_refused_and_without_an_id_logged() {
    let link = link();
    let mut client = LinkClient::connect(&link.handle());

    client.send_line(&json!({"request_id": "c", "request": {"kind": "fly"}}).to_string());
    assert!(matches!(
        reply(&mut client),
        Event::Reply {
            accepted: false,
            reason: Some(RejectionReason::Malformed),
            ..
        }
    ));
    client.send_line("not json");
    client.read_until(|event| matches!(event, Event::Rejected { .. }));
    link.close();
}

#[test]
fn a_waiting_arm_does_not_block_control_claims() {
    let link = link();
    let command_link = link.handle();
    let (started, starting) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        started.send(()).unwrap();
        command_link.submit_command_fenced(Command::Enable, None, None, None, Vec::new())
    });
    starting.recv().unwrap();
    // WHY: The link has no control loop; its command remains unanswered.
    let claim_link = link.handle();
    let (claimed, result) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = claimed.send(claim_link.acquire_lease(
            LeaseRequest {
                holder: LeaseHolder::Manual,
                label: "test".into(),
                required_cameras: Vec::new(),
                on_interruption: OnInterruption::WaitForApproval,
            },
            false,
        ));
    });
    assert!(
        result
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap()
            .is_ok()
    );
    link.close();
    assert!(worker.join().unwrap().is_err());
}

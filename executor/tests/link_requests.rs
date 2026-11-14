//! What a link answers by itself, without a control loop: control requests and
//! unreadable lines.

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

const CONFIG: &str = r#"{
  "urdf": "unused.urdf", "driven_joints": ["a", "b"], "surface_guard": null,
  "motion": {"acceleration_limit": 20.0, "jerk_limit": 200.0, "settled_error_radians": 0.02, "settle_dwell_seconds": 0.1, "settle_timeout_seconds": 2.0}, "joint_count": 2, "frequency_hertz": 100.0, "loop_deadline_seconds": 0.1,
  "lower_limits": [-2.0, -2.0], "upper_limits": [2.0, 2.0],
  "joint_effort_limits": [10.0, 10.0], "tracking_following_lag_seconds": 0.05, "tracking_static_margin_radians": 1.5,
  "max_joint_speed": 3.0, "default_joint_speed": 1.5,
  "stream_goal_timeout_seconds": 5.0,
  "joint_limit_recovery_tolerance_radians": 0.0349, "jaw_closing_axis": "x"
}"#;

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
    // Acted on again, the claim would be refused: control is already held.
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
    // The link has no control loop; its command remains unanswered.
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

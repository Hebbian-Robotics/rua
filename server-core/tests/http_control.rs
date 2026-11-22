#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    reason = "This integration-test target may abort on failed setup or unexpected outcomes."
)]

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use hebbian_robot_use_core::config::ControllerConfig;
use hebbian_robot_use_core::control_link::Link;
use hebbian_robot_use_core::control_loop::ArmSetup;
use hebbian_robot_use_core::lease::{LEASE, Lease};
use hebbian_robot_use_core::plant::{Feedback, NoPhysicsStandIn, Plant};
use hebbian_robot_use_core::station_api::DiscardTelemetry;
use hebbian_robot_use_core::station_api::HttpServer;
use serde_json::{Value, json};

mod support;
use support::{LinkClient, RunningArms};

const CONFIG: &str = include_str!("support/two_joint_config.json");

fn request(address: SocketAddr, method: &str, path: &str, body: Option<&Value>) -> (u16, Value) {
    let body = body.map(Value::to_string).unwrap_or_default();
    let mut stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    write!(stream, "{method} {path} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    let (headers, body) = response.split_once("\r\n\r\n").unwrap();
    let status = headers.split_whitespace().nth(1).unwrap().parse().unwrap();
    (status, serde_json::from_str(body).unwrap_or(Value::Null))
}

fn wait_for_state(address: SocketAddr, predicate: impl Fn(&Value) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let (status, state) = request(address, "GET", "/state", None);
        if status == 200 && predicate(&state) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "station state did not arrive: {state}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn acquire(address: SocketAddr, instance: &str, label: &str, take: bool) -> Value {
    let (status, grant) = request(
        address,
        "POST",
        "/leases",
        Some(&json!({
            "request_id": uuid::Uuid::new_v4().to_string(), "station_instance_id": instance,
            "request": {"holder":"agent", "label":label}, "take":take
        })),
    );
    assert_eq!(status, 200, "{grant}");
    grant
}

fn submit(address: SocketAddr, instance: &str, token: &str, arm: &str, command: Value) -> Value {
    let operation_id = uuid::Uuid::new_v4().to_string();
    let (status, mut answer) = request(
        address,
        "POST",
        "/commands",
        Some(&json!({
            "operation_id":operation_id, "station_instance_id":instance,
            "lease_token":token, "arm":arm, "command":command
        })),
    );
    assert_eq!(status, 200, "{answer}");
    answer["operation_id"] = json!(operation_id);
    answer
}

struct OverloadedPlant {
    inner: NoPhysicsStandIn,
    overloaded: Arc<AtomicBool>,
}

#[allow(
    clippy::disallowed_methods,
    reason = "This adapter supplies controllable feedback to the simulated plant."
)]
impl Plant for OverloadedPlant {
    fn read_feedback_into(&mut self, feedback: &mut Feedback) -> anyhow::Result<()> {
        self.inner.read_feedback_into(feedback)?;
        if self.overloaded.load(Ordering::Relaxed) {
            feedback.efforts[0] = 1000.0;
        }
        Ok(())
    }

    fn command(&mut self, positions: &[f64], seconds: f64) -> anyhow::Result<()> {
        self.inner.command(positions, seconds)
    }

    fn command_gripper(&mut self, angle: f64) -> anyhow::Result<()> {
        self.inner.command_gripper(angle)
    }

    fn preflight(&mut self) -> anyhow::Result<()> {
        self.inner.preflight()
    }

    fn enable(&mut self) -> anyhow::Result<()> {
        self.inner.enable()
    }

    fn release(&mut self) -> anyhow::Result<()> {
        self.inner.release()
    }
}

#[test]
fn station_state_reports_and_clears_a_non_hardware_protective_trip() {
    let config = ControllerConfig::parse(CONFIG).unwrap();
    let link = Link::new(&config, Arc::new(Lease::new(LEASE))).unwrap();
    let handle = link.handle();
    let mut client = LinkClient::connect(&handle);
    let server = HttpServer::start(
        "127.0.0.1:0",
        vec![],
        None,
        Arc::new(DiscardTelemetry),
        Arc::default(),
        |_, _| Ok(axum::Router::new()),
    )
    .unwrap();
    server.attach_arms(BTreeMap::from([("arm".into(), handle)]));
    let overloaded = Arc::new(AtomicBool::new(false));
    let _arms = RunningArms::start(vec![ArmSetup::new(
        "arm".into(),
        config,
        OverloadedPlant {
            inner: NoPhysicsStandIn::at(vec![0.0, 0.0], 0.02),
            overloaded: Arc::clone(&overloaded),
        },
        link,
    )]);
    client.command(hebbian_contracts::Command::Enable);
    client.read_until(|event| {
        matches!(event, hebbian_contracts::Event::State(sample) if sample.mode == hebbian_contracts::ArmMode::Holding)
    });

    let address = server.local_address();
    overloaded.store(true, Ordering::Relaxed);
    wait_for_state(address, |state| {
        let arm = &state["arms"][0];
        arm["fault"].is_null()
            && arm["active_trip"]
                .as_str()
                .is_some_and(|reason| !reason.is_empty())
            && arm["status"] == arm["active_trip"]
    });
    overloaded.store(false, Ordering::Relaxed);
    wait_for_state(address, |state| {
        let arm = &state["arms"][0];
        arm["mode"] == "holding" && arm["active_trip"].is_null()
    });
}

#[test]
fn taking_control_stops_both_arms_before_grant_and_fences_the_old_holder() {
    let config = ControllerConfig::parse(CONFIG).unwrap();
    let control = Arc::new(Lease::new(LEASE));
    let first = Link::new(&config, Arc::clone(&control)).unwrap();
    let second = Link::new(&config, control).unwrap();
    let server = HttpServer::start(
        "127.0.0.1:0",
        vec![],
        None,
        Arc::new(DiscardTelemetry),
        Arc::default(),
        |_, _| Ok(axum::Router::new()),
    )
    .unwrap();
    server.attach_arms(BTreeMap::from([
        ("first".into(), first.handle()),
        ("second".into(), second.handle()),
    ]));
    let _arms = RunningArms::start(vec![
        ArmSetup::new(
            "first".into(),
            config.clone(),
            NoPhysicsStandIn::at(vec![0.0, 0.0], 0.02),
            first,
        ),
        ArmSetup::new(
            "second".into(),
            config,
            NoPhysicsStandIn::at(vec![0.0, 0.0], 0.02),
            second,
        ),
    ]);
    let address = server.local_address();
    wait_for_state(address, |state| {
        state["arms"].as_array().is_some_and(|arms| {
            arms.len() == 2
                && arms
                    .iter()
                    .all(|arm| !arm["sampled_at_unix_seconds"].is_null())
        })
    });
    let (_, identity) = request(address, "GET", "/instance", None);
    let instance = identity["id"].as_str().unwrap();
    let grant = acquire(address, instance, "first", false);
    let old_token = grant["lease_token"].as_str().unwrap();
    for arm in ["first", "second"] {
        assert_eq!(
            submit(address, instance, old_token, arm, json!({"kind":"enable"}))["status"],
            "accepted"
        );
    }
    wait_for_state(address, |state| {
        state["arms"]
            .as_array()
            .is_some_and(|arms| arms.iter().all(|arm| arm["mode"] == "holding"))
    });
    let mut commands = Vec::new();
    for arm in ["first", "second"] {
        let answer = submit(
            address,
            instance,
            old_token,
            arm,
            json!({"kind":"stream_joints", "joints":[1.5,-1.5], "max_joint_speed":0.2}),
        );
        assert_eq!(answer["status"], "accepted", "{answer}");
        commands.push(answer["operation_id"].as_str().unwrap().to_owned());
    }
    wait_for_state(address, |state| {
        state["arms"]
            .as_array()
            .is_some_and(|arms| arms.iter().all(|arm| arm["mode"] == "moving"))
    });
    let replacement = acquire(address, instance, "replacement", true);
    assert_ne!(replacement["lease_token"], grant["lease_token"]);
    for id in commands {
        let (status, record) = request(address, "GET", &format!("/operations/{id}"), None);
        assert_eq!(status, 200);
        assert_eq!(
            record["records"][0]["record"]["phase"]["completion"]["outcome"], "stopped",
            "{record}"
        );
    }
    let old_stop = request(
        address,
        "POST",
        "/arms/first/stop",
        Some(&json!({"lease_token":old_token})),
    );
    assert_eq!(old_stop.0, 403);
}

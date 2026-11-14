//! HTTP control behavior spanning lease handoff and both arm control loops.
//! These integration checks use a simulated plant and are not hardware validation.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;
use std::time::{Duration, Instant};

use hebbian_robot_use_server::config::ExecutorConfig;
use hebbian_robot_use_server::control_link::Link;
use hebbian_robot_use_server::control_loop::ArmSetup;
use hebbian_robot_use_server::lease::{LEASE, Lease};
use hebbian_robot_use_server::plant::SimulatedPlant;
use hebbian_robot_use_server::station_api::DiscardTelemetry;
use hebbian_robot_use_server::station_api::HttpServer;
use serde_json::{Value, json};

mod support;
use support::RunningArms;

const CONFIG: &str = r#"{
  "urdf":"unused.urdf", "driven_joints":["a","b"], "surface_guard":null,
  "motion":{"acceleration_limit":20.0,"jerk_limit":200.0,"settled_error_radians":0.02,"settle_dwell_seconds":0.1,"settle_timeout_seconds":2.0},
  "joint_count":2,"frequency_hertz":100.0,"loop_deadline_seconds":0.1,
  "lower_limits":[-2.0,-2.0],"upper_limits":[2.0,2.0],"joint_effort_limits":[10.0,10.0],
  "tracking_following_lag_seconds":0.05,"tracking_static_margin_radians":1.5,
  "max_joint_speed":3.0,"default_joint_speed":1.5,"stream_goal_timeout_seconds":5.0,
  "joint_limit_recovery_tolerance_radians":0.0349,"jaw_closing_axis":"x"
}"#;

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
    let (status, answer) = request(
        address,
        "POST",
        "/commands",
        Some(&json!({
            "request_id":uuid::Uuid::new_v4().to_string(), "station_instance_id":instance,
            "lease_token":token, "arm":arm, "command":command
        })),
    );
    assert_eq!(status, 200, "{answer}");
    answer
}

#[test]
fn taking_control_stops_both_arms_before_grant_and_fences_the_old_holder() {
    let config = ExecutorConfig::parse(CONFIG).unwrap();
    let control = Arc::new(Lease::new(LEASE));
    let first = Link::new(&config, Arc::clone(&control)).unwrap();
    let second = Link::new(&config, control).unwrap();
    let server = HttpServer::start(
        "127.0.0.1:0",
        vec![],
        None,
        Arc::new(DiscardTelemetry),
        |_, _| Ok(axum::Router::new()),
    )
    .unwrap();
    server.attach_arms(BTreeMap::from([
        ("first".into(), first.handle()),
        ("second".into(), second.handle()),
    ]));
    let _arms = RunningArms::start(vec![
        ArmSetup {
            name: "first".into(),
            config: config.clone(),
            plant: SimulatedPlant::at(vec![0.0, 0.0], 0.02),
            link: Some(first),
        },
        ArmSetup {
            name: "second".into(),
            config,
            plant: SimulatedPlant::at(vec![0.0, 0.0], 0.02),
            link: Some(second),
        },
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
        commands.push(
            answer["records"][0]["record"]["command_id"]
                .as_str()
                .unwrap()
                .to_owned(),
        );
    }
    wait_for_state(address, |state| {
        state["arms"]
            .as_array()
            .is_some_and(|arms| arms.iter().all(|arm| arm["mode"] == "moving"))
    });
    let replacement = acquire(address, instance, "replacement", true);
    assert_ne!(replacement["lease_token"], grant["lease_token"]);
    for id in commands {
        let (status, record) = request(address, "GET", &format!("/commands/{id}"), None);
        assert_eq!(status, 200);
        assert_eq!(
            record["record"]["completion"]["outcome"], "stopped",
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

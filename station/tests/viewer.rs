//! The station serves the built viewer, `LiveKit` tokens, phone invites and the lease,
//! and nothing that leaves the machine can hold up control, the HTTP API
//! or the viewer. `LIVEKIT_URL` points at an address that never answers.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

use hebbian_robot_use_server::config::ExecutorConfig;
use hebbian_robot_use_server::control_link::Link;
use hebbian_robot_use_server::control_loop::run;
use hebbian_robot_use_server::lease::{LEASE, Lease};
use hebbian_robot_use_server::plant::{Feedback, Plant, RecoveryObservation, SimulatedPlant};
use hebbian_robot_use_server::station_api::HttpServer;
use hebbian_station::web::{self, WebSettings};
use serde_json::{Value, json};

const CONFIG: &str = r#"{
  "urdf": "unused.urdf", "driven_joints": ["a", "b"], "surface_guard": null,
  "motion": {"acceleration_limit": 20.0, "jerk_limit": 200.0, "settled_error_radians": 0.02, "settle_dwell_seconds": 0.1, "settle_timeout_seconds": 2.0}, "joint_count": 2, "frequency_hertz": 100.0, "loop_deadline_seconds": 0.1,
  "lower_limits": [-2.0, -2.0], "upper_limits": [2.0, 2.0],
  "joint_effort_limits": [10.0, 10.0], "tracking_following_lag_seconds": 0.05, "tracking_static_margin_radians": 1.5,
  "max_joint_speed": 3.0, "default_joint_speed": 1.5,
  "stream_goal_timeout_seconds": 5.0,
  "joint_limit_recovery_tolerance_radians": 0.0349, "jaw_closing_axis": "x"
}"#;

struct FailingPlant {
    inner: SimulatedPlant,
    failed: Arc<AtomicBool>,
    bus_down: Arc<AtomicBool>,
    torque_off: Arc<AtomicBool>,
    recovery_enables: Arc<AtomicU32>,
    releases: Arc<AtomicU32>,
}

impl Plant for FailingPlant {
    fn read_feedback_into(&mut self, feedback: &mut Feedback) -> anyhow::Result<()> {
        anyhow::ensure!(!self.failed.load(Ordering::Relaxed), "bus unavailable");
        anyhow::ensure!(!self.bus_down.load(Ordering::Relaxed), "bus unavailable");
        self.inner.read_feedback_into(feedback)
    }

    fn heartbeat(&mut self) -> anyhow::Result<()> {
        anyhow::ensure!(!self.failed.load(Ordering::Relaxed), "guard unavailable");
        Ok(())
    }

    fn observe_recovery(&mut self) -> anyhow::Result<RecoveryObservation> {
        Ok(RecoveryObservation {
            feedback: self.read_feedback()?,
            motor_torque: vec![(1, true), (2, !self.torque_off.load(Ordering::Relaxed))],
            health_issue: None,
        })
    }

    fn hold_recovery(&mut self) -> anyhow::Result<RecoveryObservation> {
        let observation = self.observe_recovery()?;
        anyhow::ensure!(
            observation.motor_torque.iter().all(|(_, enabled)| *enabled),
            "torque off"
        );
        Ok(observation)
    }

    fn enable_recovery(
        &mut self,
        _drift_limit_radians: f64,
    ) -> anyhow::Result<RecoveryObservation> {
        self.recovery_enables.fetch_add(1, Ordering::Relaxed);
        self.torque_off.store(false, Ordering::Relaxed);
        self.observe_recovery()
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
        self.releases.fetch_add(1, Ordering::Relaxed);
        self.inner.release()
    }
}

/// Status code and body of one request.
fn request(address: SocketAddr, method: &str, path: &str, body: Option<&Value>) -> (u16, String) {
    let mut stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let body = body.map(Value::to_string).unwrap_or_default();
    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nHost: station.local:8793\r\nConnection: close\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    let (head, body) = response.split_once("\r\n\r\n").unwrap();
    let status = head.split_whitespace().nth(1).unwrap().parse().unwrap();
    (status, body.to_owned())
}

fn get(address: SocketAddr, path: &str) -> (u16, String) {
    request(address, "GET", path, None)
}

fn get_json(address: SocketAddr, path: &str) -> Value {
    let (status, body) = get(address, path);
    assert_eq!(status, 200, "{path}: {body}");
    serde_json::from_str(&body).unwrap()
}

fn post_json(address: SocketAddr, path: &str, body: &Value) -> (u16, Value) {
    let (status, text) = request(address, "POST", path, Some(body));
    (status, serde_json::from_str(&text).unwrap_or(Value::Null))
}

fn scratch_directory(name: &str) -> PathBuf {
    let directory = std::env::temp_dir().join(format!("hebbian-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).unwrap();
    directory
}

fn viewer_directory() -> PathBuf {
    let directory = scratch_directory("viewer");
    std::fs::write(directory.join("index.html"), "<title>viewer</title>").unwrap();
    directory
}

/// A stand-in for `hebbian-robot-use-media` that records until it is stopped.
fn media_executable(directory: &Path) -> PathBuf {
    let path = directory.join("hebbian-robot-use-media");
    std::fs::write(&path, "#!/bin/sh\nexec sleep 30\n").unwrap();
    std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    path
}

fn wait_for_arm(address: SocketAddr, condition: impl Fn(&Value) -> bool) -> Value {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let arm = get_json(address, "/state")["arms"][0].clone();
        if condition(&arm) {
            return arm;
        }
        assert!(Instant::now() < deadline, "arm did not transition: {arm}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn has_interlock(arm: &Value, kind: &str) -> bool {
    arm["interlocks"]
        .as_array()
        .is_some_and(|interlocks| interlocks.iter().any(|interlock| interlock["kind"] == kind))
}

fn no_interlocks(arm: &Value) -> bool {
    arm["interlocks"].as_array().is_some_and(Vec::is_empty)
}

fn acquire_lease(address: SocketAddr, holder: &str, cameras: &[&str]) -> Value {
    let instance = get_json(address, "/instance");
    let (status, grant) = post_json(
        address,
        "/leases",
        &json!({
            "request_id": uuid::Uuid::new_v4().to_string(),
            "station_instance_id": instance["id"],
            "request": {"holder": holder, "label": "test", "required_cameras": cameras},
            "take": false
        }),
    );
    assert_eq!(status, 200, "{grant}");
    grant
}

fn submit_command(
    address: SocketAddr,
    token: Option<&str>,
    command: Value,
    expected_revision: Option<u64>,
) -> Value {
    let instance = get_json(address, "/instance");
    let (status, answer) = post_json(
        address,
        "/commands",
        &json!({
            "request_id": uuid::Uuid::new_v4().to_string(),
            "station_instance_id": instance["id"],
            "arm": "follower", "command": command,
            "lease_token": token, "expected_revision": expected_revision
        }),
    );
    assert_eq!(status, 200, "{answer}");
    answer
}

#[test]
#[allow(clippy::too_many_lines)]
fn tokens_leases_and_the_viewer_never_wait_on_an_unreachable_livekit() {
    // SAFETY: this is the only test in this binary, so nothing else reads the
    // environment while it is set.
    unsafe {
        std::env::set_var("LIVEKIT_URL", "wss://10.255.255.1:7880");
        std::env::set_var("LIVEKIT_API_KEY", "devkey");
        std::env::set_var(
            "LIVEKIT_API_SECRET",
            "a-secret-of-at-least-thirty-two-bytes",
        );
    }

    let mut config = ExecutorConfig::parse(CONFIG).unwrap().into_input();
    config.trace_directory = std::env::temp_dir()
        .join("hebbian-robot-use-server-test-traces")
        .to_string_lossy()
        .into_owned();
    let config = ExecutorConfig::try_from(config).unwrap();
    let link = Link::new(&config, Arc::new(Lease::new(LEASE))).unwrap();
    let scratch = scratch_directory("station");
    let settings = WebSettings {
        recordings_directory: scratch.join("recordings"),
        enable_recording: true,
        media_address: Some("127.0.0.1:8784".parse().unwrap()),
        media_executable: media_executable(&scratch),
        room: Some("station-test".into()),
        phone_page_url: web::DEFAULT_PHONE_PAGE_URL.into(),
        manual_control: false,
        media_status: hebbian_station::media_status::MediaStatus::default(),
    };
    let mut mounted = None;
    let live_run = std::sync::Arc::new(hebbian_station::run_telemetry::LiveRunStore::default());
    let server = HttpServer::start(
        "127.0.0.1:0",
        Vec::new(),
        Some(viewer_directory()),
        live_run.clone(),
        |runtime, bound| {
            let (routes, opened) = web::mount(runtime, bound, settings);
            mounted = Some(opened);
            Ok(routes.merge(hebbian_station::run_telemetry::routes(live_run)))
        },
    )
    .unwrap();
    let mounted = mounted.unwrap();
    let arms = BTreeMap::from([("follower".to_owned(), link.handle())]);
    mounted.attach_arms(&server.runtime_handle(), &arms);
    server.attach_arms(arms);
    let shutdown_requested = Arc::new(AtomicBool::new(false));
    let loop_shutdown_requested = Arc::clone(&shutdown_requested);
    let plant_failed = Arc::new(AtomicBool::new(false));
    let loop_plant_failed = Arc::clone(&plant_failed);
    let bus_down = Arc::new(AtomicBool::new(false));
    let torque_off = Arc::new(AtomicBool::new(false));
    let recovery_enables = Arc::new(AtomicU32::new(0));
    let releases = Arc::new(AtomicU32::new(0));
    let loop_bus_down = Arc::clone(&bus_down);
    let loop_torque_off = Arc::clone(&torque_off);
    let loop_recovery_enables = Arc::clone(&recovery_enables);
    let loop_releases = Arc::clone(&releases);
    let loop_thread = std::thread::spawn(move || {
        let mut plant = FailingPlant {
            inner: SimulatedPlant::at(vec![0.0, 0.0], 0.02),
            failed: loop_plant_failed,
            bus_down: loop_bus_down,
            torque_off: loop_torque_off,
            recovery_enables: loop_recovery_enables,
            releases: loop_releases,
        };
        run(
            &mut plant,
            Some(&link),
            &config,
            Some(Duration::from_secs(20)),
            &loop_shutdown_requested,
        );
        link.close();
    });
    let address = server.local_address();
    let started = Instant::now();

    // The viewer.
    let (status, page) = get(address, "/viewer/");
    assert_eq!((status, page.as_str()), (200, "<title>viewer</title>"));
    assert_eq!(get(address, "/viewer/missing.js").0, 404);
    assert_eq!(get(address, "/").0, 307, "the root redirects to the viewer");
    assert_eq!(
        get_json(address, "/viewer/api/control-mode"),
        json!({"mode": "llm"})
    );
    assert_eq!(
        get_json(address, "/viewer/api/recording-enabled"),
        json!(true)
    );
    assert_eq!(
        get_json(address, "/viewer/api/recordings")["recordings"],
        json!([])
    );

    // Tokens and phone invites are signed here, with no call to LiveKit.
    let access = get_json(address, "/viewer/api/token");
    assert_eq!(access["server_url"], "wss://10.255.255.1:7880");
    assert!(
        access["token"]
            .as_str()
            .is_some_and(|token| !token.is_empty())
    );
    let (status, invite) = post_json(
        address,
        "/viewer/api/phone-invites",
        &json!({"label": "table_left"}),
    );
    assert_eq!(status, 200, "{invite}");
    assert_eq!(invite["camera_name"], "phone_table_left");
    assert!(invite["url"].as_str().is_some_and(|url| {
        url.starts_with("https://hebbian-robotics.github.io/livekit-cameras/#")
    }));
    let (status, refusal) = post_json(
        address,
        "/viewer/api/phone-invites",
        &json!({"label": "Not A Name"}),
    );
    assert_eq!(status, 400, "{refusal}");
    assert!(refusal["detail"].is_string());

    // The room announcement goes to an address that never answers, off the request path.
    let (status, recording) = post_json(address, "/viewer/api/recording/start", &json!({}));
    assert_eq!(status, 200, "{recording}");

    // Control and state remain available while the announcement is pending.
    let (status, state) = get(address, "/state");
    assert_eq!(status, 200);
    assert!(state.contains("\"arms\""), "{state}");
    // Telemetry submitted over HTTP is visible in the live run.
    for event in [
        json!({"type": "run_started", "task": "stack"}),
        json!({"type": "notes_update", "notes": ["red first"]}),
    ] {
        assert_eq!(post_json(address, "/run/events", &event).0, 204);
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    let kept = loop {
        let live = get_json(address, "/run");
        if live["memory"]["notes"][0] == "red first" {
            break live;
        }
        assert!(Instant::now() < deadline, "the run was not kept: {live}");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(kept["memory"]["task"], "stack");
    let enabled = submit_command(address, None, json!({"kind": "enable"}), None);
    assert_eq!(enabled["status"], "accepted", "{enabled}");
    loop {
        let state = get_json(address, "/state");
        if state["arms"][0]["mode"] == "holding" {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "arm did not hold: {state}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    // No interlock yet: revision 0 is current, and a command expecting it is accepted.
    let arm = wait_for_arm(address, no_interlocks);
    assert_eq!(arm["revision"], 0);

    // An agent requiring a camera the media server does not list: the station
    // raises its interlock, which bumps the revision and refuses the agent's motion.
    let granted = acquire_lease(address, "agent", &["front"]);
    let agent_token = granted["lease_token"].as_str().unwrap();
    let arm = wait_for_arm(address, |arm| {
        has_interlock(arm, "required_camera_unavailable")
    });
    assert_eq!(arm["revision"], 1, "{arm}");
    let refused = submit_command(address, Some(agent_token), json!({"kind":"enable"}), None);
    assert_eq!(refused["reason"], "interlocked", "{refused}");
    let stale_answer = submit_command(
        address,
        Some(agent_token),
        json!({"kind":"disable"}),
        Some(0),
    );
    assert_eq!(stale_answer["reason"], "stale_revision", "{stale_answer}");
    // Only the holder can report a required camera.
    assert_eq!(
        post_json(
            address,
            "/leases/camera",
            &json!({
                "lease_token": agent_token, "camera": "front", "available": true
            })
        )
        .0,
        204
    );
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "a request waited on the network"
    );
    assert_eq!(
        get_json(address, "/viewer/api/recordings")["active"],
        recording
    );

    // Returning the lease ends its camera interlock; recording is independent of it.
    assert_eq!(
        post_json(
            address,
            "/leases/release",
            &json!({"lease_token": agent_token})
        )
        .0,
        204
    );
    wait_for_arm(address, no_interlocks);
    assert_eq!(
        get_json(address, "/viewer/api/recordings")["active"],
        recording
    );
    let (status, stopped) = post_json(address, "/viewer/api/recording/stop", &json!({}));
    assert_eq!(status, 200, "{stopped}");

    let granted = acquire_lease(address, "manual", &[]);
    let manual_token = granted["lease_token"].as_str().unwrap();
    let (status, recording) = post_json(address, "/viewer/api/recording/start", &json!({}));
    assert_eq!(status, 200, "{recording}");
    let accepted = submit_command(
        address,
        Some(manual_token),
        json!({
            "kind":"stream_joints", "joints":[1.5,-1.5], "gripper_opening":null,
            "max_joint_speed":0.2, "stream_command_id":null
        }),
        None,
    );
    assert_eq!(accepted["status"], "accepted", "{accepted}");
    let interrupted_command = accepted["records"][0]["record"]["command_id"]
        .as_str()
        .unwrap()
        .to_owned();
    wait_for_arm(address, |arm| arm["mode"] == "moving");
    let revision_before_disconnect = get_json(address, "/state")["arms"][0]["revision"]
        .as_u64()
        .unwrap();
    bus_down.store(true, Ordering::Relaxed);
    let arm = wait_for_arm(address, |arm| has_interlock(arm, "arm_disconnected"));
    assert_eq!(
        arm["revision"].as_u64(),
        Some(revision_before_disconnect + 1)
    );
    loop {
        let record = get_json(address, &format!("/commands/{interrupted_command}"));
        if !record["record"]["completion"].is_null() {
            assert_eq!(
                record["record"]["completion"]["outcome"], "interrupted",
                "{record}"
            );
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "interrupted command did not finish"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let refused = submit_command(address, Some(manual_token), json!({"kind":"enable"}), None);
    assert_eq!(refused["status"], "refused", "{refused}");
    assert_eq!(
        get_json(address, "/viewer/api/recordings")["active"],
        recording
    );
    bus_down.store(false, Ordering::Relaxed);
    wait_for_arm(address, no_interlocks);
    assert_eq!(releases.load(Ordering::Relaxed), 0);
    assert_eq!(recovery_enables.load(Ordering::Relaxed), 0);
    assert_eq!(
        get_json(address, "/viewer/api/recordings")["active"],
        recording
    );

    bus_down.store(true, Ordering::Relaxed);
    wait_for_arm(address, |arm| has_interlock(arm, "arm_disconnected"));
    torque_off.store(true, Ordering::Relaxed);
    bus_down.store(false, Ordering::Relaxed);
    let arm = wait_for_arm(address, |arm| {
        arm["interlocks"].as_array().is_some_and(|interlocks| {
            interlocks.iter().any(|interlock| {
                interlock["kind"] == "recovery_approval_required"
                    && interlock["motors"] == json!(["2"])
            })
        })
    });
    assert!(!has_interlock(&arm, "arm_disconnected"), "{arm}");
    assert_eq!(recovery_enables.load(Ordering::Relaxed), 0);
    // The operator approves, whoever holds the lease.
    let approved = submit_command(
        address,
        Some(manual_token),
        json!({"kind":"approve_recovery"}),
        None,
    );
    assert_eq!(approved["status"], "accepted", "{approved}");
    wait_for_arm(address, no_interlocks);
    assert_eq!(recovery_enables.load(Ordering::Relaxed), 1);
    assert_eq!(releases.load(Ordering::Relaxed), 0);

    // A fault powers the arm off; nothing stops the recording implicitly.
    plant_failed.store(true, Ordering::Relaxed);
    wait_for_arm(address, |arm| arm["mode"] == "faulted");
    plant_failed.store(false, Ordering::Relaxed);
    assert_eq!(
        get_json(address, "/viewer/api/recordings")["active"],
        recording
    );
    let (status, stopped) = post_json(address, "/viewer/api/recording/stop", &json!({}));
    assert_eq!(status, 200, "{stopped}");
    server.close();
    shutdown_requested.store(true, Ordering::Relaxed);
    loop_thread.join().unwrap();
}

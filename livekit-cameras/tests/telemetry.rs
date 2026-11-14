use hebbian_robot_use_server::{
    config::ExecutorConfig,
    control_link::Link,
    lease::{LEASE, Lease},
    station_api::HttpServer,
};
use hebbian_station::run_telemetry::LiveRunStore;
use std::sync::atomic::{AtomicBool, Ordering};
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use std::{fs, process::Command};

#[tokio::test]
async fn recorder_marks_controller_loss_even_if_telemetry_was_not_received()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let recording = directory.path().join("recording");
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_hebbian-robot-use-media"))
        .args(["record", "--room", "test", "--directory"])
        .arg(&recording)
        .args(["--executor-url", "http://127.0.0.1:1"])
        .env("LIVEKIT_URL", "ws://127.0.0.1:1")
        .env("LIVEKIT_API_KEY", "test-key")
        .env("LIVEKIT_API_SECRET", "test-secret")
        .kill_on_drop(true)
        .spawn()?;
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(child.try_wait()?.is_none(), "recorder started");
    fs::write(
        recording.join(hebbian_contracts::CONTROLLER_INTERRUPTION_MARKER),
        "controller unavailable",
    )?;
    let status = Command::new("kill")
        .args(["-TERM", &child.id().ok_or("recorder exited")?.to_string()])
        .status()?;
    assert!(status.success());
    assert!(
        !tokio::time::timeout(Duration::from_secs(10), child.wait())
            .await??
            .success()
    );
    let data = fs::read(recording.join("recording.mcap"))?;
    let messages = mcap::MessageStream::new(&data)?.collect::<Result<Vec<_>, _>>()?;
    let status = messages
        .iter()
        .find(|message| message.channel.topic == "/recording/status")
        .ok_or("missing recording status")?;
    let status: serde_json::Value = serde_json::from_slice(&status.data)?;
    assert_eq!(status["status"], "incomplete");
    assert!(
        status["reason"]
            .as_str()
            .unwrap_or_default()
            .contains("controller")
    );
    Ok(())
}

#[tokio::test]
async fn controller_loss_keeps_recording_and_marks_the_finished_archive_incomplete()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let config = ExecutorConfig::parse(
        r#"{
      "urdf":"unused.urdf","driven_joints":["a","b"],"surface_guard":null,
      "motion":{"acceleration_limit":20.0,"jerk_limit":200.0,"settled_error_radians":0.02,"settle_dwell_seconds":0.1,"settle_timeout_seconds":2.0},
      "joint_count":2,"frequency_hertz":100.0,"loop_deadline_seconds":0.1,
      "lower_limits":[-2.0,-2.0],"upper_limits":[2.0,2.0],"joint_effort_limits":[10.0,10.0],
      "tracking_following_lag_seconds":0.05,"tracking_static_margin_radians":1.5,"max_joint_speed":3.0,"default_joint_speed":1.5,
      "stream_goal_timeout_seconds":5.0,"joint_limit_recovery_tolerance_radians":0.0349,"jaw_closing_axis":"x"
    }"#,
    )?;
    let link = Link::new(&config, Arc::new(Lease::new(LEASE)))?;
    link.publish(hebbian_contracts::Event::ControllerAvailable);
    let server = tokio::task::spawn_blocking(|| {
        HttpServer::start(
            "127.0.0.1:0",
            vec![],
            None,
            Arc::new(LiveRunStore::default()),
            |_, _| Ok(axum::Router::new()),
        )
    })
    .await??;
    server.attach_arms(BTreeMap::from([("left".into(), link.handle())]));
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_hebbian-robot-use-media"))
        .args(["record", "--room", "test", "--directory"])
        .arg(directory.path().join("recording"))
        .args([
            "--executor-url",
            &format!("http://{}", server.local_address()),
        ])
        .env("LIVEKIT_URL", "ws://127.0.0.1:1")
        .env("LIVEKIT_API_KEY", "test-key")
        .env("LIVEKIT_API_SECRET", "test-secret")
        .kill_on_drop(true)
        .spawn()?;
    tokio::time::sleep(Duration::from_secs(2)).await;
    link.controller_unavailable("Controller exited".into());
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert!(
        child.try_wait()?.is_none(),
        "recording continues after controller loss"
    );
    link.publish(hebbian_contracts::Event::ControllerAvailable);
    tokio::time::sleep(Duration::from_millis(250)).await;
    let status = Command::new("kill")
        .args(["-TERM", &child.id().ok_or("recorder exited")?.to_string()])
        .status()?;
    assert!(status.success());
    assert!(
        !tokio::time::timeout(Duration::from_secs(10), child.wait())
            .await??
            .success()
    );
    let data = fs::read(directory.path().join("recording/recording.mcap"))?;
    let messages = mcap::MessageStream::new(&data)?.collect::<Result<Vec<_>, _>>()?;
    let controller_events = messages
        .iter()
        .filter(|message| message.channel.topic == "/station/controller")
        .map(|message| serde_json::from_slice::<serde_json::Value>(&message.data))
        .collect::<Result<Vec<_>, _>>()?;
    assert!(
        controller_events
            .iter()
            .any(|event| event["state"] == "unavailable")
    );
    assert!(
        controller_events
            .iter()
            .any(|event| event["state"] == "available")
    );
    let recording_status = messages
        .iter()
        .find(|message| message.channel.topic == "/recording/status")
        .ok_or("recording status missing")?;
    let recording_status: serde_json::Value = serde_json::from_slice(&recording_status.data)?;
    assert_eq!(recording_status["status"], "incomplete");
    assert!(
        recording_status["reason"]
            .as_str()
            .unwrap_or_default()
            .contains("controller")
    );
    tokio::task::spawn_blocking(move || server.close()).await?;
    link.close();
    Ok(())
}

#[tokio::test]
async fn unreachable_livekit_and_station_reconnect_preserve_recording()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let mut config = ExecutorConfig::parse(
        r#"{
      "urdf":"unused.urdf","driven_joints":["a","b"],"surface_guard":null,
      "motion":{"acceleration_limit":20.0,"jerk_limit":200.0,"settled_error_radians":0.02,"settle_dwell_seconds":0.1,"settle_timeout_seconds":2.0},
      "joint_count":2,"frequency_hertz":100.0,"loop_deadline_seconds":0.1,
      "lower_limits":[-2.0,-2.0],"upper_limits":[2.0,2.0],"joint_effort_limits":[10.0,10.0],
      "tracking_following_lag_seconds":0.05,"tracking_static_margin_radians":1.5,"max_joint_speed":3.0,"default_joint_speed":1.5,
      "stream_goal_timeout_seconds":5.0,"joint_limit_recovery_tolerance_radians":0.0349,"jaw_closing_axis":"x"
    }"#,
    )?.into_input();
    config.trace_directory = directory
        .path()
        .join("traces")
        .to_string_lossy()
        .into_owned();
    let config = ExecutorConfig::try_from(config)?;
    let link = Link::new(&config, Arc::new(Lease::new(LEASE)))?;
    let server = tokio::task::spawn_blocking(|| {
        HttpServer::start(
            "127.0.0.1:0",
            vec![],
            None,
            Arc::new(LiveRunStore::default()),
            |_, _| Ok(axum::Router::new()),
        )
    })
    .await??;
    let arm_links = BTreeMap::from([("left".into(), link.handle())]);
    server.attach_arms(arm_links.clone());
    let shutdown_requested = Arc::new(AtomicBool::new(false));
    let loop_shutdown_requested = Arc::clone(&shutdown_requested);
    let loop_thread = std::thread::spawn(move || {
        let mut plant = hebbian_robot_use_server::plant::SimulatedPlant::at(vec![0.0, 0.0], 0.02);
        hebbian_robot_use_server::control_loop::run(
            &mut plant,
            Some(&link),
            &config,
            Some(Duration::from_secs(8)),
            &loop_shutdown_requested,
        );
        link.close();
    });
    let executable = env!("CARGO_BIN_EXE_hebbian-robot-use-media");
    let mut child = tokio::process::Command::new(executable)
        .args(["record", "--room", "test", "--directory"])
        .arg(directory.path().join("recording"))
        .args([
            "--executor-url",
            &format!("http://{}", server.local_address()),
        ])
        .env("LIVEKIT_URL", "ws://127.0.0.1:1")
        .env("LIVEKIT_API_KEY", "test-key")
        .env("LIVEKIT_API_SECRET", "test-secret")
        .kill_on_drop(true)
        .spawn()?;
    tokio::time::sleep(Duration::from_secs(1)).await;
    send_run_history(server.local_address()).await?;
    tokio::time::sleep(Duration::from_secs(1)).await;
    let address = server.local_address().to_string();
    tokio::task::spawn_blocking(move || server.close()).await?;
    let server = tokio::task::spawn_blocking(move || {
        HttpServer::start(
            &address,
            vec![],
            None,
            Arc::new(LiveRunStore::default()),
            |_, _| Ok(axum::Router::new()),
        )
    })
    .await??;
    server.attach_arms(arm_links);
    let restarted_at = u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos(),
    )?;
    tokio::time::sleep(Duration::from_secs(2)).await;
    let status = Command::new("kill")
        .args(["-TERM", &child.id().ok_or("recorder exited")?.to_string()])
        .status()?;
    assert!(status.success());
    assert!(
        tokio::time::timeout(Duration::from_secs(10), child.wait())
            .await??
            .success()
    );
    let data = fs::read(directory.path().join("recording/recording.mcap"))?;
    let messages = mcap::MessageStream::new(&data)?.collect::<Result<Vec<_>, _>>()?;
    verify_recording(&messages, restarted_at)?;
    tokio::task::spawn_blocking(move || server.close()).await?;
    shutdown_requested.store(true, Ordering::Relaxed);
    loop_thread.join().map_err(|_| "loop thread panicked")?;
    Ok(())
}

fn verify_recording(
    messages: &[mcap::Message<'_>],
    restarted_at: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    assert!(
        messages
            .iter()
            .filter(|message| message.channel.topic == "/station/state")
            .count()
            >= 5
    );
    // Each state sample also writes the arm's joints as a ROS 2 JointState.
    let joint_state = messages
        .iter()
        .find(|message| message.channel.topic.ends_with("/joint_states"))
        .ok_or("no JointState recorded")?;
    assert_eq!(joint_state.channel.message_encoding, "cdr");
    assert_eq!(
        joint_state
            .channel
            .schema
            .as_ref()
            .map(|schema| schema.name.as_str()),
        Some("sensor_msgs/msg/JointState")
    );
    assert!(messages.iter().any(
        |message| message.channel.topic == "/station/recording_errors"
            && String::from_utf8_lossy(&message.data).contains("cameras unavailable")
    ));
    assert!(messages.iter().any(
        |message| message.channel.topic == "/station/state" && message.log_time < restarted_at
    ));
    assert!(messages.iter().any(
        |message| message.channel.topic == "/station/state" && message.log_time > restarted_at
    ));
    assert!(messages.iter().any(
        |message| message.channel.topic == "/station/recording_errors"
            && String::from_utf8_lossy(&message.data).contains("station")
    ));
    verify_run_history(messages)?;
    Ok(())
}

fn verify_run_history(messages: &[mcap::Message<'_>]) -> Result<(), Box<dyn std::error::Error>> {
    let spans = messages
        .iter()
        .filter(|message| message.channel.topic == "/trace/spans")
        .map(|message| {
            Ok((
                message.log_time,
                serde_json::from_slice::<serde_json::Value>(&message.data)?,
            ))
        })
        .collect::<Result<Vec<_>, serde_json::Error>>()?;
    assert_eq!(spans.len(), 2);
    assert_eq!(spans[0].1["endTimeUnixNano"], "0");
    assert_eq!(spans[1].1["endTimeUnixNano"], "1500000000");
    for (timestamp, span) in spans {
        assert_eq!(
            timestamp.to_string(),
            span["received_unix_nano"]
                .as_str()
                .ok_or("missing span receipt timestamp")?
        );
    }
    assert!(messages.iter().any(
        |message| message.channel.topic == "/station/recording_errors"
            && String::from_utf8_lossy(&message.data).contains("Run telemetry resume gap")
    ));
    let events = messages
        .iter()
        .filter(|message| message.channel.topic == "/model/events")
        .map(|message| {
            Ok((
                message.log_time,
                serde_json::from_slice::<serde_json::Value>(&message.data)?,
            ))
        })
        .collect::<Result<Vec<_>, serde_json::Error>>()?;
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].1["event"]["notes"][0], "first");
    assert_eq!(events[1].1["event"]["notes"][0], "second");
    assert!(events[0].1["seq"].as_u64() < events[1].1["seq"].as_u64());
    for (timestamp, event) in events {
        assert_eq!(
            timestamp.to_string(),
            event["received_unix_nano"]
                .as_str()
                .ok_or("missing receipt timestamp")?
        );
    }
    Ok(())
}

async fn send_run_history(address: std::net::SocketAddr) -> Result<(), Box<dyn std::error::Error>> {
    let client = reqwest::Client::new();
    for note in ["first", "second"] {
        client
            .post(format!("http://{address}/run/events"))
            .json(&serde_json::json!({"type": "notes_update", "notes": [note]}))
            .send()
            .await?
            .error_for_status()?;
    }
    for end in [0, 1_500_000_000_u64] {
        client.post(format!("http://{address}/run/spans"))
            .json(&serde_json::json!({"spans": [{"trace_id": "11111111111111111111111111111111", "span_id": "2222222222222222", "name": "tool call", "start_time_unix_nano": "1000000000", "end_time_unix_nano": end.to_string()}]}))
            .send().await?.error_for_status()?;
    }
    Ok(())
}

/// The station can die without a chance to stop its recorder (a crash, SIGKILL).
/// The recorder must notice, finalize a readable file, and say it is incomplete.
#[tokio::test]
async fn recorder_finalizes_as_incomplete_when_its_supervisor_is_killed()
-> Result<(), Box<dyn std::error::Error>> {
    use std::io::{BufRead, BufReader};
    let directory = tempfile::tempdir()?;
    let recording = directory.path().join("recording");
    let executable = env!("CARGO_BIN_EXE_hebbian-robot-use-media");
    // The shell stands in for the station: it starts the recorder, names itself
    // as supervisor, prints the recorder's pid and then idles until killed.
    let script = format!(
        "\"{executable}\" record --room test --directory \"{}\" \
         --executor-url http://127.0.0.1:1 --supervisor-pid $$ & echo $!; wait",
        recording.display()
    );
    let mut supervisor = Command::new("sh")
        .args(["-c", &script])
        .env("LIVEKIT_URL", "ws://127.0.0.1:1")
        .env("LIVEKIT_API_KEY", "test-key")
        .env("LIVEKIT_API_SECRET", "test-secret")
        .stdout(std::process::Stdio::piped())
        .spawn()?;
    let mut line = String::new();
    BufReader::new(supervisor.stdout.take().ok_or("no stdout")?).read_line(&mut line)?;
    let recorder_pid: i32 = line.trim().parse()?;
    tokio::time::sleep(Duration::from_secs(2)).await;
    supervisor.kill()?;
    supervisor.wait()?;
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    // `kill -0` fails once the recorder has exited.
    while Command::new("kill")
        .args(["-0", &recorder_pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()?
        .success()
    {
        assert!(
            std::time::Instant::now() < deadline,
            "the recorder outlived its supervisor"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let data = fs::read(recording.join("recording.mcap"))?;
    let messages = mcap::MessageStream::new(&data)?.collect::<Result<Vec<_>, _>>()?;
    let status = messages
        .iter()
        .rev()
        .find(|message| message.channel.topic == "/recording/status")
        .ok_or("no recording status")?;
    let status: serde_json::Value = serde_json::from_slice(&status.data)?;
    assert_eq!(status["status"], "incomplete");
    assert!(
        status["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("station"))
    );
    Ok(())
}

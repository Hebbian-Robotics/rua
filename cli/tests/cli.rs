#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    reason = "This integration-test target may abort on failed setup or unexpected outcomes."
)]

use std::path::Path;
use std::process::Output;
use std::time::Duration;

use hebbian_robot_use_core::plant::NoPhysicsStandIn;
use hebbian_rua::LeaseRequest;
use hebbian_rua::contracts::LeaseHolder;
use serde_json::Value;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[path = "../../sdk/tests/support/simulated_station.rs"]
mod simulated_station;
use simulated_station::{ARM, REST, SimulatedStation};

struct TestStation(SimulatedStation);
impl std::ops::Deref for TestStation {
    type Target = SimulatedStation;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl TestStation {
    async fn start() -> Result<Self, Box<dyn std::error::Error>> {
        SimulatedStation::start(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../profiles"),
            &[ARM],
            || NoPhysicsStandIn::at(REST.to_vec(), 0.02).with_gripper(0.0),
            axum::Router::new(),
            None,
        )
        .await
        .map(Self)
    }
}

struct Run {
    output: Output,
}

impl Run {
    fn code(&self) -> Option<i32> {
        self.output.status.code()
    }

    fn stdout_json(&self) -> Value {
        serde_json::from_slice(&self.output.stdout).expect("stdout is one JSON value")
    }

    fn stderr_json(&self) -> Value {
        serde_json::from_slice(&self.output.stderr).expect("stderr is one JSON value")
    }
}

async fn cli(server: &str, arguments: &[&str]) -> Run {
    let output = tokio::time::timeout(
        Duration::from_secs(60),
        tokio::process::Command::new(env!("CARGO_BIN_EXE_hebbian-robot-use"))
            .args(["--server", server])
            .args(arguments)
            .stdin(std::process::Stdio::null())
            .output(),
    )
    .await
    .expect("the command finishes")
    .expect("the binary runs");
    Run { output }
}

async fn ready_station() -> Result<TestStation, Box<dyn std::error::Error>> {
    let station = TestStation::start().await?;
    station.connect().await?;
    Ok(station)
}

#[tokio::test(flavor = "multi_thread")]
async fn state_prints_one_json_value() -> TestResult {
    let station = ready_station().await?;
    let run = cli(&station.url, &["state"]).await;
    assert_eq!(run.code(), Some(0));
    let state = run.stdout_json();
    assert_eq!(state["arms"][0]["arm"], ARM);
    assert_eq!(state["joint_names"].as_array().map(Vec::len), Some(5));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn output_is_compact_by_default_and_raw_keeps_si_records() -> TestResult {
    let station = ready_station().await?;
    let compact = cli(&station.url, &["state"]).await.stdout_json();
    let arm = &compact["arms"][0];
    assert!(arm["measured_joints_degrees"].is_array());
    assert!(arm["tool"]["position_mm"].is_array());
    assert!(arm.get("measured_joints").is_none());
    assert!(arm.get("last_command").is_none(), "history is left out");
    assert!(compact["description"].is_string(), "frames stay in `state`");
    assert!(
        arm["tool"].get("model_points_mm").is_none(),
        "render geometry is left out"
    );
    assert!(
        arm.get("commanded_joints_degrees").is_none(),
        "nulls are omitted"
    );

    let raw = cli(&station.url, &["--raw", "state"]).await.stdout_json();
    let arm = &raw["arms"][0];
    assert!(arm["measured_joints"].is_array());
    assert!(arm["tool"]["position"].is_array());
    assert!(arm["tool"]["model_points"].is_array());
    assert!(arm["commanded_joints"].is_null());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn enable_then_move_to_joints_completes() -> TestResult {
    let station = ready_station().await?;
    assert_eq!(cli(&station.url, &["enable", ARM]).await.code(), Some(0));
    let run = cli(
        &station.url,
        &["move-joints", ARM, "--joints", "0.2,-0.5,0.5,0.0,0.0"],
    )
    .await;
    assert_eq!(
        run.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&run.output.stderr)
    );
    let moved = run.stdout_json();
    assert_eq!(moved["completion"]["outcome"], "completed");
    assert!(moved.get("trace_context").is_none() && moved.get("accepted_at_unix_nano").is_none());
    let run = cli(&station.url, &["rehome", ARM]).await;
    assert_eq!(
        run.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&run.output.stderr)
    );
    assert_eq!(run.stdout_json()["completion"]["outcome"], "completed");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refused_command_exits_1_with_the_reason() -> TestResult {
    let station = ready_station().await?;
    let run = cli(&station.url, &["move-joints", ARM, "--joints", "0,0,0,0,0"]).await;
    assert_eq!(run.code(), Some(1));
    let error = run.stderr_json();
    assert_eq!(error["error"]["kind"], "refused");
    assert_eq!(error["error"]["reason"], "not_enabled");
    assert!(
        error["error"]["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("enable"))
    );
    assert_eq!(run.output.stdout.len(), 0);
    assert_eq!(
        cli(&station.url, &["move-joints", ARM, "--joints", "0,0,0,0,0"])
            .await
            .code(),
        Some(1)
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn connecting_to_nothing_exits_3() {
    let run = cli("http://127.0.0.1:1", &["state"]).await;
    assert_eq!(run.code(), Some(3));
    assert_eq!(run.stderr_json()["error"]["kind"], "connection");
}

#[tokio::test(flavor = "multi_thread")]
async fn lease_held_elsewhere_exits_4_with_the_holder() -> TestResult {
    let station = ready_station().await?;
    let holder = station.connect().await?;
    let _guard = holder
        .acquire_lease(LeaseRequest::new(LeaseHolder::Agent, "someone else"))
        .await?;
    let run = cli(&station.url, &["enable", ARM]).await;
    assert_eq!(run.code(), Some(4));
    let error = run.stderr_json();
    assert_eq!(error["error"]["kind"], "lease_held_elsewhere");
    assert_eq!(error["error"]["holder"]["label"], "someone else");
    let state = cli(&station.url, &["state"]).await.stdout_json();
    assert_eq!(state["lease"]["label"], "someone else");
    let moved = cli(&station.url, &["move-joints", ARM, "--joints", "0,0,0,0,0"]).await;
    assert_eq!(moved.code(), Some(4));

    assert_eq!(
        cli(&station.url, &["lease", "revoke"]).await.code(),
        Some(0)
    );
    assert_eq!(cli(&station.url, &["enable", ARM]).await.code(), Some(0));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn preview_pose_projects_without_moving() -> TestResult {
    let station = ready_station().await?;
    assert_eq!(cli(&station.url, &["enable", ARM]).await.code(), Some(0));
    let run = cli(
        &station.url,
        &[
            "preview-pose",
            ARM,
            "--position",
            "0.2,0.0,0.1",
            "--approach",
            "down",
            "--jaw-yaw-degrees",
            "0",
        ],
    )
    .await;
    assert_eq!(
        run.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&run.output.stderr)
    );
    assert!(run.stdout_json().is_object());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn preview_poses_answers_every_pose_in_order() -> TestResult {
    let station = ready_station().await?;
    assert_eq!(cli(&station.url, &["enable", ARM]).await.code(), Some(0));
    let file = std::env::temp_dir().join(format!(
        "hebbian-cli-preview-poses-{}.json",
        std::process::id()
    ));
    std::fs::write(
        &file,
        serde_json::json!([
            {"label": "near", "position": [0.2, 0.0, 0.1], "orientation": {"kind": "grasp", "approach": [0.0, 0.0, -1.0], "jaw_yaw_degrees": 90.0}},
            {"label": "far", "position": [3.0, 3.0, 3.0]}
        ])
        .to_string(),
    )?;
    let run = cli(
        &station.url,
        &["preview-poses", ARM, &file.to_string_lossy()],
    )
    .await;
    assert_eq!(
        run.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&run.output.stderr)
    );
    let results = run.stdout_json();
    let labels: Vec<_> = results
        .as_array()
        .unwrap()
        .iter()
        .map(|result| result["label"].clone())
        .collect();
    assert_eq!(
        labels,
        [serde_json::json!("near"), serde_json::json!("far")]
    );
    std::fs::remove_file(file)?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_action_sequence_runs_from_a_file() -> TestResult {
    let station = ready_station().await?;
    assert_eq!(cli(&station.url, &["enable", ARM]).await.code(), Some(0));
    let file = std::env::temp_dir().join(format!(
        "hebbian-cli-action_sequence-{}.json",
        std::process::id()
    ));
    let step = |joints: [f64; 5]| serde_json::json!({"arms": {ARM: {"movement": {"kind": "joints", "joints": joints}}}});
    std::fs::write(
        &file,
        serde_json::json!({"steps": [step([0.2, -0.5, 0.5, 0.0, 0.0]), step([-0.2, -0.5, 0.5, 0.0, 0.0])]})
            .to_string(),
    )?;
    let run = cli(
        &station.url,
        &["action-sequence", "run", &file.to_string_lossy()],
    )
    .await;
    assert_eq!(
        run.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&run.output.stderr)
    );
    assert_eq!(run.stdout_json()["phase"]["state"], "completed");
    std::fs::remove_file(file)?;
    Ok(())
}

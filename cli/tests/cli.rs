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

use hebbian_rua::LeaseRequest;
use hebbian_rua::contracts::LeaseHolder;
use hebbian_rua_core::plant::NoPhysicsStandIn;
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

    async fn with_media(address: std::net::SocketAddr) -> Result<Self, Box<dyn std::error::Error>> {
        SimulatedStation::start(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../profiles"),
            &[ARM],
            || NoPhysicsStandIn::at(REST.to_vec(), 0.02).with_gripper(0.0),
            axum::Router::new(),
            Some(address),
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
        tokio::process::Command::new(env!("CARGO_BIN_EXE_hebbian-rua"))
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

#[tokio::test(flavor = "multi_thread")]
async fn output_is_compact_by_default_and_raw_keeps_si_records() -> TestResult {
    let station = TestStation::start().await?;
    let state = cli(&station.url, &["state"]).await;
    assert_eq!(state.code(), Some(0));
    let compact = state.stdout_json();
    let arm = &compact["arms"][0];
    assert_eq!(arm["arm"], ARM);
    assert_eq!(compact["joint_names"].as_array().map(Vec::len), Some(5));
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
async fn observation_and_compilation_workflows_use_station_state() -> TestResult {
    let station = TestStation::start().await?;
    let observation = cli(&station.url, &["observation", "--arm", ARM]).await;
    assert_eq!(
        observation.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&observation.output.stderr)
    );
    assert_eq!(observation.stdout_json()["state"]["arms"][0]["arm"], ARM);

    let source_path =
        std::env::temp_dir().join(format!("rua-cli-sequence-{}.json", std::process::id()));
    let artifact_path =
        std::env::temp_dir().join(format!("rua-cli-artifact-{}.json", std::process::id()));
    std::fs::write(
        &source_path,
        format!(
            r#"{{"steps":[{{"arms":{{"{ARM}":{{"goal":{{"joints_degrees":[0,-30,30,0,0]}}}}}}}}]}}"#
        ),
    )?;
    let compiled = cli(
        &station.url,
        &["compilation", "compile", source_path.to_str().unwrap()],
    )
    .await;
    assert_eq!(
        compiled.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&compiled.output.stderr)
    );
    let artifact = compiled.stdout_json();
    let id = artifact["compilation_id"].as_str().unwrap();
    std::fs::write(&artifact_path, &compiled.output.stdout)?;
    let inspected = cli(&station.url, &["compilation", "inspect", id]).await;
    assert_eq!(inspected.code(), Some(0));
    assert_eq!(inspected.stdout_json()["compilation_id"], id);
    let imported = cli(
        &station.url,
        &["compilation", "import", artifact_path.to_str().unwrap()],
    )
    .await;
    assert_eq!(
        imported.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&imported.output.stderr)
    );
    assert_eq!(cli(&station.url, &["enable", ARM]).await.code(), Some(0));
    let executed = cli(&station.url, &["compilation", "run", id]).await;
    assert_eq!(
        executed.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&executed.output.stderr)
    );
    assert_eq!(executed.stdout_json()["phase"]["state"], "completed");
    std::fs::remove_file(source_path)?;
    std::fs::remove_file(artifact_path)?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn sequence_camera_history_is_saved_after_a_run() -> TestResult {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let station = TestStation::with_media(listener.local_addr()?).await?;
    let media = axum::Router::new().route(
        "/cameras/{name}",
        axum::routing::get(|| async {
            (
                [(axum::http::header::CONTENT_TYPE, "image/jpeg")],
                "example frame",
            )
        }),
    );
    let media_task = tokio::spawn(async move { axum::serve(listener, media).await });
    let directory = std::env::temp_dir().join(format!("rua-cli-history-{}", std::process::id()));
    std::fs::create_dir_all(&directory)?;
    let source = directory.join("steps.json");
    std::fs::write(
        &source,
        format!(r#"{{"steps":[{{"arms":{{"{ARM}":{{"gripper_opening":0.5}}}}}}]}}"#),
    )?;
    assert_eq!(cli(&station.url, &["enable", ARM]).await.code(), Some(0));
    let run = cli(
        &station.url,
        &[
            "action-sequence",
            "run",
            source.to_str().unwrap(),
            "--camera",
            "front",
        ],
    )
    .await;
    assert_eq!(
        run.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&run.output.stderr)
    );
    let result = run.stdout_json();
    let id = result["execution_id"].as_str().unwrap();
    let output = directory.join("images");
    let mut history = Value::Null;
    for _ in 0..50 {
        let fetched = cli(
            &station.url,
            &[
                "action-sequence",
                "images",
                id,
                "--output",
                output.to_str().unwrap(),
            ],
        )
        .await;
        assert_eq!(
            fetched.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&fetched.output.stderr)
        );
        history = fetched.stdout_json();
        if !history.is_null() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let images = history["images"].as_array().expect("retained history");
    let first_image = images.first().expect("at least one captured frame");
    assert_eq!(first_image["camera"], "front");
    assert_eq!(
        std::fs::read(first_image["path"].as_str().unwrap())?,
        b"example frame"
    );
    assert!(first_image["elapsed_seconds"].is_number());
    assert!(history["missed_samples"].is_number());
    std::fs::remove_dir_all(directory)?;
    media_task.abort();
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn enable_then_move_to_joints_completes() -> TestResult {
    let station = TestStation::start().await?;
    assert_eq!(cli(&station.url, &["enable", ARM]).await.code(), Some(0));
    let run = cli(
        &station.url,
        &["move-joints", ARM, "--joints-degrees", "10,-30,30,0,0"],
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
    let state = cli(&station.url, &["--raw", "state"]).await.stdout_json();
    let measured = state["arms"][0]["measured_joints"][0]
        .as_f64()
        .expect("state reports the measured joint angle");
    assert!((measured.to_degrees() - 10.0).abs() < 1.0);
    assert!(moved.get("trace_context").is_none() && moved.get("accepted_unix_nano").is_none());
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
    let station = TestStation::start().await?;
    let run = cli(
        &station.url,
        &["move-joints", ARM, "--joints-radians", "0,0,0,0,0"],
    )
    .await;
    assert_eq!(run.code(), Some(1));
    let error = run.stderr_json();
    assert_eq!(error["error"]["kind"], "refused");
    assert_eq!(error["error"]["reason"], "not_enabled");
    assert!(
        error["error"]["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("enable"))
    );
    assert_eq!(run.output.stdout.as_slice(), b"");
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
    let station = TestStation::start().await?;
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
    let moved = cli(
        &station.url,
        &["move-joints", ARM, "--joints-radians", "0,0,0,0,0"],
    )
    .await;
    assert_eq!(moved.code(), Some(4));

    assert_eq!(
        cli(&station.url, &["lease", "revoke"]).await.code(),
        Some(0)
    );
    assert_eq!(cli(&station.url, &["enable", ARM]).await.code(), Some(0));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn preview_pose_prints_the_projected_pose() -> TestResult {
    let station = TestStation::start().await?;
    assert_eq!(cli(&station.url, &["enable", ARM]).await.code(), Some(0));
    let preview_arguments = [
        "preview-pose",
        ARM,
        "--position-mm",
        "200,0,100",
        "--approach",
        "down",
        "--jaw-yaw-degrees",
        "90",
    ];
    let compact = cli(&station.url, &preview_arguments).await;
    assert_eq!(compact.code(), Some(0));
    let result = compact.stdout_json();
    assert_eq!(result["outcome"]["kind"], "projected");
    assert!(result["outcome"]["pose"]["position_mm"].is_array());

    let raw_arguments: Vec<_> = ["--raw"].into_iter().chain(preview_arguments).collect();
    let raw = cli(&station.url, &raw_arguments).await;
    assert_eq!(raw.code(), Some(0));
    let yaw = raw.stdout_json()["requested"]["orientation"]["jaw_yaw_radians"]
        .as_f64()
        .expect("native preview includes the requested jaw yaw");
    assert!((yaw - std::f64::consts::FRAC_PI_2).abs() < 1e-9);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn preview_poses_answers_every_pose_in_order() -> TestResult {
    let station = TestStation::start().await?;
    assert_eq!(cli(&station.url, &["enable", ARM]).await.code(), Some(0));
    let file = std::env::temp_dir().join(format!(
        "hebbian-cli-preview-poses-{}.json",
        std::process::id()
    ));
    std::fs::write(
        &file,
        serde_json::json!([
            {"label": "near", "position_mm": [200, 0, 100], "orientation": {"approach": [0, 0, -1], "jaw_yaw_degrees": 90}},
            {"label": "far", "position_mm": [3000, 3000, 3000]}
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
    std::fs::write(&file, r#"[{"label":"native","position":[0.2,0.0,0.1]}]"#)?;
    let path = file.to_string_lossy();
    let rejected = cli(&station.url, &["preview-poses", ARM, &path]).await;
    assert_eq!(rejected.code(), Some(2));
    assert!(
        rejected.stderr_json()["error"]["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("use explicit model units for position"))
    );
    let native = cli(
        &station.url,
        &["preview-poses", ARM, "--input-station", &path],
    )
    .await;
    assert_eq!(native.code(), Some(0));
    assert_eq!(native.stdout_json()[0]["label"], "native");

    for station_orientation in [
        serde_json::json!({"kind": "grasp", "approach": "down", "jaw_yaw_degrees": 90}),
        serde_json::json!({
            "kind": "grasp",
            "approach": "down",
            "jaw_yaw_radians": std::f64::consts::FRAC_PI_2,
            "jaw_yaw_degrees": 0
        }),
    ] {
        std::fs::write(
            &file,
            serde_json::json!([{
                "position": [0.2, 0.0, 0.1],
                "orientation": station_orientation,
            }])
            .to_string(),
        )?;
        let rejected = cli(
            &station.url,
            &["preview-poses", ARM, "--input-station", &path],
        )
        .await;
        assert_eq!(rejected.code(), Some(2));
        assert!(
            rejected.stderr_json()["error"]["detail"]
                .as_str()
                .is_some_and(|detail| detail.contains("jaw_yaw_degrees"))
        );
    }
    std::fs::remove_file(file)?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_timed_action_sequence_runs_from_a_model_unit_file() -> TestResult {
    let station = TestStation::start().await?;
    assert_eq!(cli(&station.url, &["enable", ARM]).await.code(), Some(0));
    let file = std::env::temp_dir().join(format!(
        "hebbian-cli-action_sequence-{}.json",
        std::process::id()
    ));
    std::fs::write(
        &file,
        serde_json::json!({"steps": [
            {"arms": {ARM: {
                "waypoints": [{
                    "at": {"joints_degrees": [5.0, -30.0, 30.0, 0.0, 0.0]},
                    "at_seconds": 1.0
                }],
                "goal": {"joints_degrees": [10.0, -30.0, 30.0, 0.0, 0.0]},
                "at_seconds": 3.0
            }}},
            {"arms": {ARM: {"goal": {"joints_degrees": [-10.0, -30.0, 30.0, 0.0, 0.0]}}}}
        ]})
        .to_string(),
    )?;
    let run = cli(
        &station.url,
        &["--raw", "action-sequence", "run", &file.to_string_lossy()],
    )
    .await;
    assert_eq!(
        run.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&run.output.stderr)
    );
    let record: hebbian_rua::contracts::ActionSequenceRecord =
        serde_json::from_value(run.stdout_json())?;
    assert_eq!(
        record.phase,
        hebbian_rua::contracts::ActionSequencePhase::Completed
    );
    let hebbian_rua::contracts::ActionSequenceArmAction::Move {
        at_seconds,
        waypoints,
        ..
    } = &record.requested_steps[0].arms[ARM]
    else {
        panic!("expected a timed move");
    };
    assert_eq!(*at_seconds, Some(3.0));
    assert_eq!(waypoints[0].at_seconds, Some(1.0));
    let first_move = &record.steps[0][0].record;
    let hebbian_rua::contracts::CommandPhase::Finished { completion, .. } = &first_move.phase
    else {
        panic!("expected the timed move to finish");
    };
    assert!(completion.finished_unix_nano.0 - first_move.accepted_unix_nano.0 >= 2_900_000_000);
    std::fs::remove_file(file)?;
    Ok(())
}

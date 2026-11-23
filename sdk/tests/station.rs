#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    reason = "This integration-test target may abort on failed setup or unexpected outcomes."
)]

//! The SDK against a real in-process controller server driving the simulated plant.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use hebbian_robot_use_core::plant::{Feedback, NoPhysicsStandIn, Pacing, Plant, PlantWritePermit};
use hebbian_rua::contracts::{
    ActionSequenceArmAction, ActionSequenceGoal, ActionSequencePhase, ActionSequenceRequest,
    ActionSequenceStep, CommandOutcome, CompilationOutcome, GripperResult, LeaseHolder,
    PoseOrientation, PosePreviewBatchRequest, PosePreviewCandidate, PreviewAnswer, PreviewPose,
    ProjectionOutcome, RejectionReason, RunEvent, TraceContext,
};
use hebbian_rua::{
    CommandOptions, ConnectOptions, Error, LeaseRequest, RobotToolCall, RobotToolOutcome,
    RobotTools, RunUpdate, Station,
};
use serde_json::json;

type TestResult = Result<(), Box<dyn std::error::Error>>;

mod support {
    pub mod simulated_station;
}
use support::simulated_station::{ARM, REST, SimulatedStation};

/// The simulated plant with something between the jaws: closing stops at `blocked_at`.
struct ObstructedJawPlant {
    inner: NoPhysicsStandIn,
    blocked_at: Option<f64>,
}

impl Plant for ObstructedJawPlant {
    fn pacing(&self) -> Pacing {
        self.inner.pacing()
    }

    fn read_feedback_into(&mut self, feedback: &mut Feedback) -> anyhow::Result<()> {
        self.inner.read_feedback_into(feedback)
    }

    fn command(
        &mut self,
        permit: &PlantWritePermit,
        positions: &[f64],
        seconds: f64,
    ) -> anyhow::Result<()> {
        self.inner.command(permit, positions, seconds)
    }

    fn command_gripper(&mut self, permit: &PlantWritePermit, angle: f64) -> anyhow::Result<()> {
        self.inner.command_gripper(
            permit,
            self.blocked_at
                .map_or(angle, |blocked_at| angle.max(blocked_at)),
        )
    }

    fn preflight(&mut self) -> anyhow::Result<()> {
        self.inner.preflight()
    }

    fn enable(&mut self, permit: &PlantWritePermit) -> anyhow::Result<()> {
        self.inner.enable(permit)
    }

    fn release(&mut self, permit: &PlantWritePermit) -> anyhow::Result<()> {
        self.inner.release(permit)
    }
}

/// The bundled SO-101 on the simulated plant, served over HTTP.
struct TestStation {
    runtime: SimulatedStation,
    /// The HTTP requests the stub lease and recording routes received.
    requests: StubRequests,
}

impl TestStation {
    async fn start() -> Result<Self, Box<dyn std::error::Error>> {
        Self::start_with_jaw_blocked_at(None).await
    }

    /// As `start`, with the jaw unable to close past `blocked_at` radians.
    async fn start_with_jaw_blocked_at(
        blocked_at: Option<f64>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        Self::start_with_arms(&[ARM], blocked_at).await
    }

    async fn start_with_arms(
        arm_names: &[&str],
        blocked_at: Option<f64>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        Self::start_with_media(arm_names, blocked_at, None).await
    }

    async fn start_with_media(
        arm_names: &[&str],
        blocked_at: Option<f64>,
        media_address: Option<std::net::SocketAddr>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let requests = StubRequests::default();
        let runtime = SimulatedStation::start(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../profiles"),
            arm_names,
            move || ObstructedJawPlant {
                inner: NoPhysicsStandIn::at(REST.to_vec(), 0.02)
                    .with_gripper(if blocked_at.is_some() { 1.0 } else { 0.0 }),
                blocked_at,
            },
            stub_station_routes(requests.clone()),
            media_address,
        )
        .await?;
        Ok(Self { runtime, requests })
    }
}
impl std::ops::Deref for TestStation {
    type Target = SimulatedStation;
    fn deref(&self) -> &Self::Target {
        &self.runtime
    }
}

fn agent(label: &str) -> LeaseRequest {
    LeaseRequest::new(LeaseHolder::Agent, label)
}

async fn within<T>(future: impl Future<Output = T>) -> Result<T, tokio::time::error::Elapsed> {
    tokio::time::timeout(Duration::from_secs(20), future).await
}

#[tokio::test(flavor = "multi_thread")]
async fn state_lists_the_arm_and_no_holder() -> TestResult {
    let station = TestStation::start().await?;
    let client = station.connect().await?;
    let state = within(client.state()).await??;
    assert_eq!(state.joint_names.len(), 5);
    assert_eq!(
        state
            .arms
            .iter()
            .map(|arm| arm.arm.as_str())
            .collect::<Vec<_>>(),
        [ARM]
    );
    assert!(state.lease.is_none());
    drop(client);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn connecting_to_nothing_is_a_connection_error() -> TestResult {
    let result = Station::connect("http://127.0.0.1:1").await;
    assert!(matches!(result, Err(Error::Connection(_))));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn the_lease_is_exclusive_revoked_and_returned() -> TestResult {
    let station = TestStation::start().await?;
    let first = station.connect().await?;
    let second = station.connect().await?;

    let guard = within(first.acquire_lease(agent("first"))).await??;
    assert_eq!(guard.status().label, "first");
    let Err(Error::Connection(detail)) = within(first.acquire_lease(agent("overlapping"))).await?
    else {
        return Err("the same client must refuse a second lease while holding one".into());
    };
    assert!(detail.contains("already holds or is acquiring"));
    let Err(Error::LeaseHeldElsewhere(Some(holder))) =
        within(second.acquire_lease(agent("second"))).await?
    else {
        return Err("a second claim must be refused with the holder".into());
    };
    assert_eq!(holder.label, "first");

    // The operator override takes control and tells the previous holder.
    within(second.revoke_lease()).await??;
    within(guard.lost()).await?;

    // An awaited return permits an immediate claim by another connection.
    let new_guard = within(first.acquire_lease(agent("again"))).await??;
    within(guard.lost()).await?;
    let stale_tools = RobotTools::new(&guard, ARM).with_operations(
        &[hebbian_rua::contracts::RobotToolOperation::SetGripper]
            .into_iter()
            .collect(),
    );
    let stale_gripper = within(stale_tools.execute(
        RobotToolCall {
            id: "stale".into(),
            name: "set_arm_gripper".into(),
            arguments: json!({"arm": ARM, "opening": 0.5}),
        },
        CommandOptions::default(),
    ))
    .await??;
    assert!(matches!(
        stale_gripper.outcome,
        RobotToolOutcome::Refused {
            reason: RejectionReason::LeaseHeldElsewhere,
            ..
        }
    ));
    within(new_guard.return_lease()).await??;
    let guard = within(second.acquire_lease(agent("returned"))).await??;
    within(guard.return_lease()).await??;

    // Dropping a guard also attempts a background release.
    let guard = within(first.acquire_lease(agent("drop"))).await??;
    drop(guard);
    within(async {
        loop {
            match second.acquire_lease(agent("next")).await {
                Err(Error::LeaseHeldElsewhere(_)) => tokio::task::yield_now().await,
                other => break other,
            }
        }
    })
    .await??;
    drop((first, second));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_joint_move_completes_and_the_arm_rests_at_the_target() -> TestResult {
    let station = TestStation::start().await?;
    let client = station.connect().await?;
    let lease = within(client.acquire_lease(agent("test"))).await??;

    within(client.enable(ARM)).await??;
    let record = within(lease.move_to_joints(ARM, vec![0.3, -0.4, 0.6, 0.1, 0.0], None)).await??;
    assert_eq!(record.outcome(), Some(CommandOutcome::Completed));

    let state = within(client.state()).await??;
    let arm = &state.arms[0];
    assert!((arm.measured_joints[0] - 0.3).abs() < 0.05);
    assert!((arm.measured_joints[1] + 0.4).abs() < 0.05);
    drop((lease, client));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn refused_commands_surface_their_reason() -> TestResult {
    let station = TestStation::start().await?;
    let holder = station.connect().await?;
    let other = station.connect().await?;

    // The lease does not need the arm powered; a move on a disabled arm is refused with the way on.
    let lease = within(holder.acquire_lease(agent("holder"))).await??;
    let Err(Error::Refused { reason, detail }) =
        within(lease.move_to_joints(ARM, vec![0.1, -0.5, 0.5, 0.0, 0.0], None)).await?
    else {
        return Err("a move before enable must be refused".into());
    };
    assert_eq!(reason, RejectionReason::NotEnabled);
    assert!(detail.contains("enable"), "{detail}");

    // Another connection cannot take the lease without taking it over.
    assert!(matches!(
        within(other.acquire_lease(agent("other"))).await?,
        Err(Error::LeaseHeldElsewhere(_))
    ));

    // The wrong number of joints never moves the arm.
    within(holder.enable(ARM)).await??;
    let Err(Error::Refused { reason, .. }) =
        within(lease.move_to_joints(ARM, vec![0.1], None)).await?
    else {
        return Err("the wrong joint count must be refused".into());
    };
    assert_eq!(reason, RejectionReason::InvalidTarget);
    drop((lease, holder, other));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stopped_move_is_an_incomplete_error() -> TestResult {
    let station = TestStation::start().await?;
    let client = station.connect().await?;
    let lease = within(client.acquire_lease(agent("test"))).await??;
    within(client.enable(ARM)).await??;

    // A slow move that a stop from another connection cuts short.
    let stopper = station.connect().await?;
    let moving = lease.move_to_joints(ARM, vec![1.0, -0.5, 0.5, 0.0, 0.0], Some(0.05.try_into()?));
    let stopping = async {
        // Stop once the move is the arm's active command.
        loop {
            let state = stopper.state().await?;
            if state.arms[0].active_command.is_some() {
                break;
            }
            tokio::task::yield_now().await;
        }
        stopper.stop(ARM).await
    };
    let (moved, stop_sent) = Box::pin(within(async { tokio::join!(moving, stopping) })).await?;
    stop_sent?;
    let Err(Error::Incomplete { outcome, .. }) = moved else {
        return Err("the move must end incomplete".into());
    };
    assert_eq!(outcome, CommandOutcome::Stopped);
    drop((lease, client, stopper));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refused_command_does_not_stop_the_holders_motion() -> TestResult {
    let station = TestStation::start().await?;
    let former = station.connect().await?;
    let holder = station.connect().await?;
    let former_lease = within(former.acquire_lease(agent("former"))).await??;
    within(former.enable(ARM)).await??;
    let lease = within(holder.take_lease(agent("holder"))).await??;

    // The holder starts a slow move that is still under way when the former holder's
    // guard, whose lease was taken, probes.
    let moving = lease.move_to_joints(ARM, vec![1.0, -0.5, 0.5, 0.0, 0.0], Some(0.05.try_into()?));
    let probing = async {
        former
            .wait_for_state(|state| state.arms[0].active_command.is_some())
            .await?;
        // A refused command moved nothing, so it must not send the stop a dropped command
        // sends: a stop is accepted from anyone and would cut short the holder's motion.
        let probe = former_lease
            .move_to_joints(ARM, vec![0.1, -0.5, 0.5, 0.0, 0.0], None)
            .await;
        assert!(matches!(probe, Err(Error::Refused { .. })), "{probe:?}");
        // The server handles one connection's messages in order, so once this answer is
        // back any stop sent before it has been handled.
        let tool = former.state().await?.arms[0]
            .tool
            .clone()
            .ok_or("the arm reports no tool pose")?;
        former
            .preview_pose(
                ARM,
                PreviewPose {
                    position: tool.position,
                    orientation: None,
                },
            )
            .await?;
        let ended = tokio::time::timeout(
            Duration::from_millis(300),
            former.wait_for_state(|state| state.arms[0].active_command.is_none()),
        )
        .await;
        assert!(ended.is_err(), "the holder's move was cut short");
        holder.stop(ARM).await?;
        Ok::<(), Box<dyn std::error::Error>>(())
    };
    let (moved, probed) = Box::pin(within(async { tokio::join!(moving, probing) })).await?;
    probed?;
    assert!(matches!(moved, Err(Error::Incomplete { .. })));
    drop((former_lease, lease, former, holder));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn compiled_sequence_can_run_and_continue_under_a_live_lease() -> TestResult {
    let station = TestStation::start().await?;
    let client = station.connect().await?;
    let joints_step = |joints: Vec<f64>| {
        ActionSequenceStep::single(
            ARM,
            ActionSequenceArmAction::Move {
                waypoints: vec![],
                goal: ActionSequenceGoal::Joints {
                    joints,
                    max_joint_speed: None,
                },
                gripper_opening: None,
                max_joint_speed: None,
            },
        )
    };
    let source = ActionSequenceRequest {
        steps: vec![
            joints_step(vec![0.2, -0.5, 0.5, 0.0, 0.0]),
            joints_step(vec![-0.2, -0.5, 0.5, 0.0, 0.0]),
        ],
        expected_revisions: BTreeMap::default(),
    };
    let CompilationOutcome::Compiled { artifact } =
        within(client.compile_action_sequence(&source)).await??
    else {
        return Err("valid source failed compilation".into());
    };
    let state = client.get_state().await?;
    assert!(state.lease.is_none());
    assert!(state.arms.iter().all(|arm| arm.active_command.is_none()));
    let inspection = client.inspect_compilation(&artifact.compilation_id).await?;
    assert_eq!(inspection.source, source);

    let lease = within(client.acquire_lease(agent("test"))).await??;
    let disabled =
        within(lease.run_compiled_sequence(&artifact.compilation_id, BTreeMap::new())).await??;
    assert!(matches!(
        disabled.phase,
        ActionSequencePhase::Stopped {
            failure: hebbian_rua::contracts::ExecutionFailure::Preparation {
                step: 0,
                reason: RejectionReason::NotEnabled,
                ..
            }
        }
    ));
    assert_eq!(disabled.steps.len(), 0);
    within(client.enable(ARM)).await??;
    let explicit =
        within(lease.run_compiled_sequence(&artifact.compilation_id, BTreeMap::new())).await??;
    assert_eq!(explicit.phase, ActionSequencePhase::Completed);
    assert_eq!(explicit.steps.len(), 2);
    assert_eq!(
        explicit.steps[1][0].record.destination_joints(),
        Some([-0.2, -0.5, 0.5, 0.0, 0.0].as_slice())
    );
    let mut revised_source = source;
    revised_source
        .steps
        .push(joints_step(vec![0.1, -0.5, 0.5, 0.0, 0.0]));
    let CompilationOutcome::Compiled { artifact: revised } =
        client.compile_action_sequence(&revised_source).await?
    else {
        return Err("appended program failed compilation".into());
    };
    let continuation = hebbian_rua::contracts::ProgramContinuation {
        prior_execution_id: explicit.id.clone(),
        next_step: 2,
    };
    let continuation_id = lease
        .continue_compiled_sequence(
            &revised.compilation_id,
            BTreeMap::new(),
            continuation.clone(),
        )
        .await?;
    let continued = within(client.wait_for_action_sequence(&continuation_id)).await??;
    assert_eq!(continued.phase, ActionSequencePhase::Completed);
    assert_eq!(continued.continuation, Some(continuation));
    assert_eq!(continued.steps.len(), 1);
    assert_eq!(
        continued.steps[0][0].record.destination_joints(),
        Some([0.1, -0.5, 0.5, 0.0, 0.0].as_slice())
    );
    let repeated = lease
        .continue_compiled_sequence(
            &revised.compilation_id,
            BTreeMap::new(),
            hebbian_rua::contracts::ProgramContinuation {
                prior_execution_id: explicit.id.clone(),
                next_step: 1,
            },
        )
        .await;
    assert!(matches!(repeated, Err(Error::Http { status: 409, .. })));
    assert_ne!(disabled.id, explicit.id);
    drop((lease, client));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn imported_compilations_rebuild_source_without_trusting_saved_operations() -> TestResult {
    let station = TestStation::start().await?;
    let client = station.connect().await?;
    let source = ActionSequenceRequest {
        steps: vec![ActionSequenceStep::single(
            ARM,
            ActionSequenceArmAction::Gripper {
                gripper_opening: 0.8,
                max_joint_speed: None,
            },
        )],
        expected_revisions: BTreeMap::new(),
    };
    let CompilationOutcome::Compiled { artifact } = client.compile_action_sequence(&source).await?
    else {
        return Err("valid source failed compilation".into());
    };
    let mut exported = *artifact;
    exported.compiler_version = "previous-compiler".into();
    exported.station_instance_id = "another-station".into();
    exported.compiled.steps.clear();
    exported.targets.clear();
    let CompilationOutcome::Compiled { artifact: imported } =
        client.import_compilation(&exported).await?
    else {
        return Err("valid exported source failed import".into());
    };
    assert_ne!(imported.compilation_id, exported.compilation_id);
    assert_eq!(imported.source, source);
    assert_eq!(imported.compiled.steps.len(), 1);
    assert_eq!(
        imported.compiled.steps[0][ARM].action,
        hebbian_rua::contracts::CompiledArmAction::Gripper {
            opening: 0.8.try_into().unwrap(),
            max_joint_speed: None,
        }
    );
    assert!(client.get_state().await?.lease.is_none());
    assert_eq!(exported.schema_version, 3);
    let mut legacy = serde_json::to_value(&exported)?;
    legacy["schema_version"] = json!(1);
    legacy["source"]["steps"][0]["arms"][ARM]["goal"] = serde_json::Value::Null;
    let rejected = reqwest::Client::new()
        .post(format!("{}/compilations/import", station.url))
        .json(&legacy)
        .send()
        .await?;
    assert_eq!(rejected.status(), reqwest::StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        rejected
            .text()
            .await?
            .contains("Unsupported compilation schema")
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn compiled_submission_retries_share_one_physical_execution() -> TestResult {
    let station = TestStation::start().await?;
    let client = station.connect().await?;
    let source = ActionSequenceRequest {
        steps: vec![ActionSequenceStep::single(
            ARM,
            ActionSequenceArmAction::Gripper {
                gripper_opening: 0.8,
                max_joint_speed: None,
            },
        )],
        expected_revisions: BTreeMap::new(),
    };
    let CompilationOutcome::Compiled { artifact } = client.compile_action_sequence(&source).await?
    else {
        return Err("valid source failed compilation".into());
    };
    within(client.enable(ARM)).await??;
    let transport = reqwest::Client::new();
    let instance = client.get_station_instance().await?;
    let grant: hebbian_rua::contracts::LeaseGrant = transport
        .post(format!("{}/leases", station.url))
        .json(&json!({
            "request_id": uuid::Uuid::new_v4().to_string(),
            "station_instance_id": instance.id,
            "take": false,
            "request": {
                "holder": "agent",
                "label": "submission-retry-test",
                "required_cameras": [],
                "on_interruption": "wait_for_approval",
            }
        }))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let submission = hebbian_rua::contracts::RunCompiledProgramRequest {
        continuation: None,
        request_id: uuid::Uuid::new_v4().to_string(),
        station_instance_id: instance.id,
        compilation_id: artifact.compilation_id,
        lease_token: grant.lease_token,
        expected_revisions: BTreeMap::new(),
    };
    let submit = || {
        transport
            .post(format!("{}/compilations/run", station.url))
            .json(&submission)
            .send()
    };
    let (first, retry) = tokio::join!(submit(), submit());
    let first_id: String = first?.error_for_status()?.json().await?;
    let retry_id: String = retry?.error_for_status()?.json().await?;
    assert_eq!(first_id, retry_id);
    let completed = within(client.wait_for_action_sequence(&first_id)).await??;
    assert_eq!(completed.phase, ActionSequencePhase::Completed);
    assert_eq!(completed.steps.len(), 1);
    let command_id = completed.steps[0][0].record.command_id.clone();

    let retried_id: String = submit().await?.error_for_status()?.json().await?;
    assert_eq!(retried_id, first_id);
    let after_retry = client.get_action_sequence(&retried_id).await?;
    assert_eq!(after_retry.steps[0][0].record.command_id, command_id);

    let mut conflicting = submission;
    conflicting.compilation_id = uuid::Uuid::new_v4().to_string();
    let refused = transport
        .post(format!("{}/compilations/run", station.url))
        .json(&conflicting)
        .send()
        .await?;
    assert_eq!(refused.status(), reqwest::StatusCode::CONFLICT);
    assert!(refused.text().await?.contains("different work"));
    let missing = transport
        .get(format!(
            "{}/compilations/{}",
            station.url, conflicting.compilation_id
        ))
        .send()
        .await?;
    assert_eq!(missing.status(), reqwest::StatusCode::NOT_FOUND);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_preview_answers_without_moving_the_arm() -> TestResult {
    let station = TestStation::start().await?;
    let client = station.connect().await?;
    let without_lease = client
        .preview_poses(&PosePreviewBatchRequest {
            arm: "unknown".into(),
            poses: vec![PosePreviewCandidate {
                id: uuid::Uuid::new_v4().to_string(),
                label: None,
                pose: PreviewPose {
                    position: [0.0; 3],
                    orientation: None,
                },
            }],
        })
        .await?;
    assert!(matches!(
        without_lease.results[0].answer,
        PreviewAnswer::Refused { .. }
    ));
    let _lease = within(client.acquire_lease(agent("test"))).await??;
    within(client.enable(ARM)).await??;
    let before = within(client.state()).await??.arms.remove(0);
    let reachable = PreviewPose {
        position: before
            .tool
            .as_ref()
            .ok_or("the arm reports no tool pose")?
            .position,
        orientation: None,
    };
    let (valid_preview, unknown_arm_preview) = tokio::join!(
        client.preview_pose(ARM, reachable.clone()),
        client.preview_pose("unknown", reachable.clone()),
    );
    assert!(valid_preview.is_ok());
    assert!(matches!(unknown_arm_preview, Err(Error::Refused { .. })));

    assert_http_pose_preview_batch(&client, reachable).await?;

    // An unreachable pose is still a projection; an unknown arm is a refusal.
    within(client.preview_pose(
        ARM,
        PreviewPose {
            position: [50.0, 50.0, 50.0],
            orientation: None,
        },
    ))
    .await??;
    let after = within(client.state()).await??.arms.remove(0);
    assert_eq!(after.mode, before.mode);
    Ok(())
}

async fn assert_http_pose_preview_batch(client: &Station, reachable: PreviewPose) -> TestResult {
    let candidates = [
        "00000000-0000-4000-8000-000000000001",
        "00000000-0000-4000-8000-000000000002",
    ];
    let batch = within(
        client.preview_poses(&PosePreviewBatchRequest {
            arm: ARM.into(),
            poses: candidates
                .iter()
                .enumerate()
                .map(|(index, id)| PosePreviewCandidate {
                    id: (*id).to_owned(),
                    label: (index == 0).then(|| "approach".into()),
                    pose: if index == 1 {
                        PreviewPose {
                            position: [0.2, 0.0, 0.2],
                            orientation: Some(PoseOrientation::Quaternion {
                                quaternion: [0.0; 4],
                            }),
                        }
                    } else {
                        reachable.clone()
                    },
                })
                .collect(),
        }),
    )
    .await??;
    assert_eq!(
        batch
            .results
            .iter()
            .map(|item| item.id.as_str())
            .collect::<Vec<_>>(),
        candidates
    );
    assert!(
        batch
            .results
            .iter()
            .all(|item| matches!(item.answer, PreviewAnswer::Projected { .. }))
    );
    assert_eq!(batch.results[0].label.as_deref(), Some("approach"));
    let PreviewAnswer::Projected { projection } = &batch.results[1].answer else {
        return Err("invalid orientation did not report an outcome".into());
    };
    assert!(matches!(
        projection.outcome,
        ProjectionOutcome::Unavailable { .. }
    ));
    let duplicate = PosePreviewCandidate {
        id: candidates[0].to_owned(),
        label: None,
        pose: PreviewPose {
            position: [0.2, 0.0, 0.2],
            orientation: None,
        },
    };
    let invalid = client
        .preview_poses(&PosePreviewBatchRequest {
            arm: ARM.into(),
            poses: vec![duplicate.clone(), duplicate],
        })
        .await;
    assert!(matches!(invalid, Err(Error::Http { status: 400, .. })));

    Ok(())
}

fn note(text: &str) -> Result<RunEvent, serde_json::Error> {
    serde_json::from_value(serde_json::json!({"type": "notes_update", "notes": [text]}))
}

async fn next_event(
    updates: &mut tokio::sync::broadcast::Receiver<RunUpdate>,
) -> Result<(u64, RunEvent), Box<dyn std::error::Error>> {
    loop {
        if let RunUpdate::Event { seq, event, .. } = updates.recv().await? {
            return Ok((seq, event));
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn run_telemetry_resumes_after_the_last_action_sequence_number() -> TestResult {
    let station = TestStation::start().await?;
    let producer = station.connect().await?;
    within(producer.state()).await??;
    producer.record_run_event(note("first")?).await?;
    producer.record_run_event(note("second")?).await?;

    let options = |run_after| ConnectOptions {
        server: station.url.clone(),
        run_after,
        ..ConnectOptions::default()
    };
    let (watcher, mut updates) = Station::connect_with(options(Some(0))).await?;
    let (first_seq, first) = within(next_event(&mut updates)).await??;
    let (second_seq, second) = within(next_event(&mut updates)).await??;
    assert_eq!(first, note("first")?);
    assert_eq!(second, note("second")?);
    assert!(first_seq < second_seq);

    producer.record_run_event(note("third")?).await?;
    let (third_seq, _) = within(next_event(&mut updates)).await??;

    // A new connection resuming after the second sees only the third.
    let (resumed_station, mut resumed_updates) =
        Station::connect_with(options(Some(second_seq))).await?;
    let (resumed_seq, resumed_event) = within(next_event(&mut resumed_updates)).await??;
    assert_eq!(resumed_seq, third_seq);
    assert_eq!(resumed_event, note("third")?);
    drop((producer, watcher, resumed_station));
    Ok(())
}

// ---- the lease and recording routes, stubbed ------------------------------------------

type StubRequests = Arc<std::sync::Mutex<Vec<(String, String, serde_json::Value)>>>;

/// Stands in for the station's `/viewer/api` lease and recording routes: logs each
/// request and answers with the shapes the real ones return.
fn stub_station_routes(requests: StubRequests) -> axum::Router {
    async fn answer(
        axum::extract::State(requests): axum::extract::State<StubRequests>,
        request: axum::extract::Request,
    ) -> axum::Json<serde_json::Value> {
        let (parts, body) = request.into_parts();
        let bytes = axum::body::to_bytes(body, usize::MAX)
            .await
            .unwrap_or_default();
        let body = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        let (method, path) = (parts.method.to_string(), parts.uri.path().to_owned());
        let mut log = requests.lock().expect("the stub log lock");
        let started = log
            .iter()
            .any(|(_, seen, _)| seen.ends_with("/recording/start"));
        let reply = match (method.as_str(), path.as_str()) {
            ("POST", path) if path.ends_with("/recording/start") => "rec-1".into(),
            ("GET", path) if path.ends_with("/recording/status") && started => {
                serde_json::json!({"state": "recording", "name": "rec-1"})
            }
            ("GET", path) if path.ends_with("/recording/status") => {
                serde_json::json!({"state": "idle"})
            }
            ("GET", path) if path.ends_with("/media-status") => {
                serde_json::json!({
                    "service": {"state": "running"},
                    "cameras": {
                        "wrist": {"state": "disconnected"},
                        "front": {"state": "capturing", "publication": {"state": "publishing"}}
                    }
                })
            }
            _ => serde_json::Value::Null,
        };
        log.push((method, path, body));
        axum::Json(reply)
    }
    axum::Router::new()
        .route("/viewer/api/{*rest}", axum::routing::any(answer))
        .with_state(requests)
}

fn requests_seen(station: &TestStation) -> Vec<(String, String, serde_json::Value)> {
    station.requests.lock().expect("the stub log lock").clone()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_lease_sends_its_token_with_its_recording_route() -> TestResult {
    let station = TestStation::start().await?;
    let client = station.connect().await?;
    let lease = within(client.acquire_lease(agent("test"))).await??;
    assert_eq!(within(client.ensure_recording()).await??, "rec-1");
    within(lease.stop_recording()).await??;
    let seen: Vec<_> = requests_seen(&station)
        .into_iter()
        .filter(|(_, path, _)| path.ends_with("/stop-for-lease"))
        .collect();
    let [(_, recording_path, recording)] = seen.as_slice() else {
        return Err(format!("unexpected requests {seen:?}").into());
    };
    assert_eq!(recording_path, "/viewer/api/recording/stop-for-lease");
    assert!(recording["lease_token"].is_string());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn commands_carry_a_trace_context_and_an_expected_revision() -> TestResult {
    let station = TestStation::start().await?;
    let client = station.connect().await?;
    let lease = within(client.acquire_lease(agent("test"))).await??;
    let trace_context = TraceContext {
        traceparent: "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01".to_owned(),
        tracestate: None,
    };
    let record = within(client.enable_with(
        ARM,
        CommandOptions {
            trace_context: Some(trace_context.clone()),
            ..CommandOptions::default()
        },
    ))
    .await??;
    assert_eq!(record.trace_context, Some(trace_context));

    let expecting = |revision| CommandOptions {
        expected_revisions: BTreeMap::from([(ARM.to_owned(), revision)]),
        ..CommandOptions::default()
    };
    let current = within(client.state()).await??.arms[0].revision;
    let stale =
        within(lease.move_to_joints_with(ARM, REST.to_vec(), None, expecting(current + 1))).await?;
    assert!(matches!(
        stale,
        Err(Error::Refused {
            reason: RejectionReason::StaleRevision,
            ..
        })
    ));
    within(lease.move_to_joints_with(ARM, REST.to_vec(), None, expecting(current))).await??;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cancelled_action_sequence_stops_and_is_published_on_the_updates() -> TestResult {
    let station = TestStation::start().await?;
    let client = station.connect().await?;
    let lease = within(client.acquire_lease(agent("test"))).await??;
    within(client.enable(ARM)).await??;
    let mut updates = client.action_sequence_updates();
    let id = within(lease.start_action_sequence(ActionSequenceRequest {
        steps: vec![ActionSequenceStep::single(
            ARM,
            ActionSequenceArmAction::Move {
                waypoints: vec![],
                goal: ActionSequenceGoal::Joints {
                    joints: vec![1.0, -0.5, 0.5, 0.0, 0.0],
                    max_joint_speed: Some(0.05),
                },
                gripper_opening: None,
                max_joint_speed: None,
            },
        )],
        expected_revisions: BTreeMap::default(),
    }))
    .await??;
    within(client.cancel_action_sequence(&id)).await??;
    let ended = within(client.wait_for_action_sequence(&id)).await??;
    assert!(matches!(ended.phase, ActionSequencePhase::Stopped { .. }));
    let published = within(updates.wait_for(|action_sequence| {
        action_sequence.as_ref().is_some_and(|action_sequence| {
            action_sequence.id == id && action_sequence.phase == ended.phase
        })
    }))
    .await??;
    drop(published);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn taking_the_lease_stops_the_previous_holders_action_sequence() -> TestResult {
    let station = TestStation::start().await?;
    let first = station.connect().await?;
    let second = station.connect().await?;
    let first_lease = within(first.acquire_lease(agent("first"))).await??;
    within(first.enable(ARM)).await??;
    let id = within(first_lease.start_action_sequence(ActionSequenceRequest {
        steps: vec![ActionSequenceStep::single(
            ARM,
            ActionSequenceArmAction::Move {
                waypoints: vec![],
                goal: ActionSequenceGoal::Joints {
                    joints: vec![1.0, -0.5, 0.5, 0.0, 0.0],
                    max_joint_speed: Some(0.05),
                },
                gripper_opening: None,
                max_joint_speed: None,
            },
        )],
        expected_revisions: BTreeMap::default(),
    }))
    .await??;
    within(first.wait_for_state(|state| state.arms[0].active_command.is_some())).await??;
    let _replacement = within(second.take_lease(agent("replacement"))).await??;
    let outcome = within(first.wait_for_action_sequence(&id)).await??;
    assert!(matches!(outcome.phase, ActionSequencePhase::Stopped { .. }));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn an_action_sequence_stops_after_a_limited_joint_target() -> TestResult {
    let station = TestStation::start().await?;
    let client = station.connect().await?;
    let lease = within(client.acquire_lease(agent("test"))).await??;
    within(client.enable(ARM)).await??;
    let steps = [vec![100.0, -0.5, 0.5, 0.0, 0.0], REST.to_vec()]
        .into_iter()
        .map(|joints| {
            ActionSequenceStep::single(
                ARM,
                ActionSequenceArmAction::Move {
                    waypoints: vec![],
                    goal: ActionSequenceGoal::Joints {
                        joints,
                        max_joint_speed: None,
                    },
                    gripper_opening: None,
                    max_joint_speed: None,
                },
            )
        })
        .collect();
    let id = within(lease.start_action_sequence(ActionSequenceRequest {
        steps,
        expected_revisions: BTreeMap::default(),
    }))
    .await??;
    let outcome = within(client.wait_for_action_sequence(&id)).await??;
    assert!(matches!(outcome.phase, ActionSequencePhase::Stopped { .. }));
    assert!(outcome.steps.len() < 2);
    Ok(())
}

async fn assert_model_tool_batches(tools: &RobotTools<&hebbian_rua::LeaseGuard>) -> TestResult {
    let call = |name: &str, arguments| RobotToolCall {
        id: "call-1".into(),
        name: name.into(),
        arguments,
    };
    let batch = tools
        .execute_calls(
            vec![
                call("observe", json!({"wait_seconds": 0.01})),
                call(
                    "preview_pose",
                    json!({"poses": [{"pose": {"position_mm": [200.0, 0.0, 200.0]}}]}),
                ),
                call("observe", json!({"wait_seconds": 11})),
            ],
            CommandOptions::default(),
        )
        .await;
    assert_eq!(batch.len(), 3);
    assert!(matches!(batch[0].outcome, RobotToolOutcome::Returned(_)));
    assert!(matches!(batch[1].outcome, RobotToolOutcome::Returned(_)));
    assert!(matches!(
        batch[2].outcome,
        RobotToolOutcome::InvalidInput(_)
    ));
    let completed = within(tools.execute_calls(
        vec![
            call(
                "set_arm_gripper",
                json!({"arm": ARM, "opening": 0.4, "max_joint_speed_degrees_per_second": 999.0_f64.to_degrees()}),
            ),
            call("observe", json!({"wait_seconds": 0})),
        ],
        CommandOptions::default(),
    ))
    .await?;
    let RobotToolOutcome::Returned(gripper) = &completed[0].outcome else {
        return Err("gripper batch operation did not complete".into());
    };
    assert_eq!(gripper["phase"]["completion"]["outcome"], "completed");
    assert_eq!(gripper["phase"]["completion"]["gripper"]["kind"], "reached");
    let profile_speed = tools.context().await?.station["max_joint_speed_degrees_per_second"]
        .as_f64()
        .ok_or("missing speed")?;
    let expected_speed = (profile_speed * 10.0).round() / 10.0;
    assert_eq!(
        completed[0].model_data()["joint_speed_degrees_per_second"],
        expected_speed
    );
    assert!(matches!(
        completed[1].outcome,
        RobotToolOutcome::Returned(_)
    ));
    let trace_context = TraceContext {
        traceparent: "00-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-bbbbbbbbbbbbbbbb-01".into(),
        tracestate: None,
    };
    let traced_stop = tools
        .execute_calls(
            vec![call("stop", json!({}))],
            CommandOptions {
                trace_context: Some(trace_context.clone()),
                ..CommandOptions::default()
            },
        )
        .await;
    let RobotToolOutcome::Returned(stop) = &traced_stop[0].outcome else {
        return Err("traced stop did not complete".into());
    };
    // The trace context reaches the station (see the command-level test); the model view omits it.
    assert!(stop.get("trace_context").is_some());
    assert!(traced_stop[0].model_data().get("trace_context").is_none());
    Ok(())
}

async fn assert_constrained_tool_move(
    tools: &RobotTools<&hebbian_rua::LeaseGuard>,
    client: &Station,
) -> TestResult {
    let examples: serde_json::Value = serde_json::from_str(include_str!(
        "../../tests/fixtures/model-tool-simulation.json"
    ))?;
    let tool_pose = client.state().await?.arms[0]
        .tool
        .clone()
        .ok_or("missing tool pose")?;
    let limited = within(tools.execute(
        RobotToolCall { id: "limited".into(), name: "move_arms".into(), arguments: json!({"goals": { (ARM): {"position_mm": examples["limited_position_millimeters"], "orientation": {"quaternion": tool_pose.quaternion}}}}) },
        CommandOptions::default(),
    )).await??;
    let RobotToolOutcome::Incomplete(limited) = limited.outcome else {
        return Err("constrained move was not reported as incomplete".into());
    };
    assert_eq!(limited["status"], "limited");
    Ok(())
}

async fn assert_model_pose_previews(
    client: &Station,
    lease: &hebbian_rua::LeaseGuard,
    tools: &RobotTools<&hebbian_rua::LeaseGuard>,
) -> TestResult {
    let call = |name: &str, arguments| RobotToolCall {
        id: "call-1".into(),
        name: name.into(),
        arguments,
    };
    let preview_position = client.state().await?.arms[0]
        .tool
        .as_ref()
        .ok_or("missing tool pose")?
        .position;
    let preview = tools
        .execute(
            call("preview_pose", json!({"poses": [
                {"pose": {"position_mm": preview_position.map(|coordinate| coordinate * 1000.0)}},
                {"pose": {"position_mm": [50000.0, 50000.0, 50000.0]}}
            ]})),
            CommandOptions::default(),
        )
        .await?;
    let RobotToolOutcome::Returned(projection) = &preview.outcome else {
        return Err("preview did not return a projection".into());
    };
    let preview_result = preview.model_data();
    assert!(
        preview_result["results"][0]["answer"]["projection"]["outcome"]["pose"]["position_mm"]
            .is_array()
    );
    assert!(
        preview_result["results"]
            .as_array()
            .ok_or("missing results")?
            .iter()
            .all(|candidate| candidate.get("id").is_none() && candidate.get("label").is_none())
    );
    assert!(
        preview_result["results"][0]["answer"]["projection"]["outcome"]["pose"]
            .get("position")
            .is_none()
    );
    assert_eq!(
        projection["results"][1]["answer"]["projection"]["outcome"]["kind"],
        "projected"
    );
    assert_eq!(
        projection["results"][1]["answer"]["projection"]["outcome"]["within_tolerance"],
        false
    );
    let absent_arm = RobotTools::new(lease, "not-an-arm").with_operations(
        &hebbian_rua::contracts::robot_tool_catalog()
            .into_iter()
            .map(|tool| tool.operation)
            .collect(),
    );
    let refused = absent_arm
        .execute(
            call("preview_pose", json!({"poses": [{"pose": {"position_mm": preview_position.map(|coordinate| coordinate * 1000.0)}}]})),
            CommandOptions::default(),
        )
        .await?;
    let RobotToolOutcome::Returned(refused) = refused.outcome else {
        return Err("expected per-candidate refusal".into());
    };
    assert_eq!(refused["results"][0]["answer"]["status"], "refused");
    assert!(
        absent_arm
            .definitions()
            .iter()
            .any(|definition| definition.name == "set_arm_gripper"
                && definition.description != "Custom gripper description")
    );
    Ok(())
}

async fn assert_specialized_model_tools(
    tools: &RobotTools<&hebbian_rua::LeaseGuard>,
) -> TestResult {
    let specialized = tools
        .clone()
        .map_tools(hebbian_rua::contracts::default_robot_tool_mapping)?
        .map_tools(|capabilities| capabilities.into_iter().rev().collect())?
        .map_tools(hebbian_rua::contracts::filter_sequence_actions(|action| {
            action != hebbian_rua::contracts::SequenceAction::Joints
        }))?;
    assert_eq!(
        specialized
            .definitions()
            .iter()
            .map(|tool| tool.name)
            .collect::<Vec<_>>(),
        ["observe", "stop", "preview_pose", "action_sequence"],
    );
    assert!(
        tools
            .definitions()
            .iter()
            .any(|tool| tool.name == "set_arm_gripper")
    );
    assert!(
        !specialized
            .definitions()
            .iter()
            .any(|tool| tool.name == "set_arm_gripper")
    );
    let sequence_schema = specialized
        .definitions()
        .into_iter()
        .find(|tool| tool.name == "action_sequence")
        .ok_or("missing sequence tool")?
        .parameters
        .to_string();
    assert!(!sequence_schema.contains("joints_degrees"));
    let rejected = specialized.execute(RobotToolCall {
        id: "hidden-joints".into(), name: "action_sequence".into(),
        arguments: serde_json::json!({"steps": [{"goal": {"joints_degrees": [0, 0, 0, 0, 0]}}]}),
    }, CommandOptions::default()).await?;
    assert!(matches!(
        rejected.outcome,
        RobotToolOutcome::InvalidInput(_)
    ));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn model_tools_execute_against_the_simulated_station() -> TestResult {
    let station = TestStation::start().await?;
    let client = station.connect().await?;
    let lease = within(client.acquire_lease(agent("model tools"))).await??;
    within(client.enable(ARM)).await??;
    let tools = RobotTools::new(&lease, ARM)
        .with_operations(
            &hebbian_rua::contracts::robot_tool_catalog()
                .into_iter()
                .map(|tool| tool.operation)
                .collect(),
        )
        .with_descriptions(BTreeMap::from([(
            "set_arm_gripper".into(),
            "Custom gripper description".into(),
        )]));
    assert_specialized_model_tools(&tools).await?;
    let context = tools.context().await?;
    assert!(context.conventions["orientation"].is_string());
    assert!(tools.definitions().iter().any(|definition| {
        definition.name == "set_arm_gripper"
            && definition.description == "Custom gripper description"
    }));
    let call = |name: &str, arguments| RobotToolCall {
        id: "call-1".into(),
        name: name.into(),
        arguments,
    };
    assert_model_tool_batches(&tools).await?;
    assert_model_pose_previews(&client, &lease, &tools).await?;
    let invalid_gripper = tools
        .execute(
            call("set_arm_gripper", json!({"arm": ARM, "opening": "half"})),
            CommandOptions::default(),
        )
        .await?;
    assert!(matches!(
        invalid_gripper.outcome,
        RobotToolOutcome::InvalidInput(_)
    ));
    assert_constrained_tool_move(&tools, &client).await?;
    let before_rehome = within(client.state()).await??.arms.remove(0);
    let opening_before_rehome = before_rehome
        .gripper_opening
        .ok_or("missing gripper opening")?;
    assert!(opening_before_rehome > 0.02);
    assert!(
        before_rehome
            .measured_joints
            .iter()
            .zip(REST)
            .any(|(actual, expected)| (actual - expected).abs() >= 0.02)
    );
    let rehomed =
        within(tools.execute(call("rehome", json!({})), CommandOptions::default())).await??;
    let RobotToolOutcome::Returned(home_record) = rehomed.outcome else {
        return Err("expected completed rehome".into());
    };
    assert_eq!(home_record["label"], "rehome");
    let joints = home_record["phase"]["achieved"]["joints"]
        .as_array()
        .ok_or("missing achieved joints")?;
    assert!(joints.iter().zip(REST).all(|(actual, expected)| {
        actual
            .as_f64()
            .is_some_and(|actual| (actual - expected).abs() < 0.02)
    }));
    let arm = within(client.state()).await??.arms.remove(0);
    assert!(
        arm.gripper_opening
            .is_some_and(|opening| (opening - opening_before_rehome).abs() < 0.02)
    );
    assert!(
        arm.measured_joints
            .iter()
            .zip(REST)
            .all(|(actual, expected)| (actual - expected).abs() < 0.02)
    );
    let goal = within(tools.execute(
        call(
            "action_sequence",
            json!({"steps": [{"goal": {"joints_degrees": REST.map(f64::to_degrees)}}]}),
        ),
        CommandOptions::default(),
    ))
    .await??;
    let RobotToolOutcome::Returned(sequence) = goal.outcome else {
        return Err("sequence did not return".into());
    };
    assert_eq!(sequence["phase"]["state"], "completed");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_gripper_command_reports_whether_the_jaw_arrived_or_stopped_short() -> TestResult {
    const BLOCKED_AT_RADIANS: f64 = 0.3;
    let station = TestStation::start_with_jaw_blocked_at(Some(BLOCKED_AT_RADIANS)).await?;
    let client = station.connect().await?;
    let lease = within(client.acquire_lease(agent("jaw"))).await??;
    within(client.enable(ARM)).await??;

    let opened = within(lease.set_gripper(ARM, 0.5.try_into()?, None)).await??;
    if !matches!(
        opened.phase,
        hebbian_contracts::CommandPhase::Finished {
            completion: hebbian_contracts::CommandCompletion {
                gripper: Some(GripperResult::Reached { .. }),
                ..
            },
            ..
        }
    ) {
        return Err(format!("opening should arrive: {:?}", opened.phase).into());
    }
    let closed = within(lease.set_gripper(ARM, 0.0.try_into()?, None)).await??;
    let hebbian_contracts::CommandPhase::Finished { completion, .. } = closed.phase else {
        return Err("the close did not finish".into());
    };
    assert_eq!(
        completion.outcome,
        hebbian_contracts::KnownCommandOutcome::Completed
    );
    let Some(GripperResult::StoppedShort {
        measured_angle_radians,
        target_angle_radians,
    }) = completion.gripper
    else {
        return Err(format!("closing on an obstruction should stop short: {completion:?}").into());
    };
    assert!((measured_angle_radians - BLOCKED_AT_RADIANS).abs() < 0.05);
    assert!(target_angle_radians < measured_angle_radians);
    assert!(
        completion
            .detail
            .is_some_and(|detail| detail.contains("short of"))
    );
    lease.return_lease().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_camera_without_a_frame_reports_the_cameras_listed_and_the_latest_failure() -> TestResult
{
    let station = TestStation::start().await?;
    let media = axum::Router::new()
        .route(
            "/cameras",
            axum::routing::get(|| async {
                axum::Json(serde_json::json!([{"name": "wrist", "available": false}]))
            }),
        )
        .route(
            "/cameras/{name}",
            axum::routing::get(|| async { axum::http::StatusCode::NOT_FOUND }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let media_server = format!("http://{}", listener.local_addr()?);
    let media_task = tokio::spawn(async move { axum::serve(listener, media).await });
    let (client, _) = Station::connect_with(hebbian_rua::ConnectOptions {
        server: station.url.clone(),
        media_server,
        run_after: None,
    })
    .await?;
    let Err(Error::CameraUnavailable {
        name,
        listed,
        last_error,
    }) = within(client.camera_jpeg("wrist")).await?
    else {
        return Err("expected CameraUnavailable".into());
    };
    assert_eq!(name, "wrist");
    assert_eq!(listed.len(), 1);
    assert_eq!(
        last_error.as_deref(),
        Some("USB camera disconnected (no matching device)")
    );
    media_task.abort();
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn multi_arm_sequences_admit_whole_steps_and_report_each_arms_motion_and_gripper()
-> TestResult {
    use hebbian_rua::contracts::ActionSequenceArmAction;
    let station = TestStation::start_with_arms(&["left", "right"], None).await?;
    let client = station.connect().await?;
    let lease = within(client.acquire_lease(agent("multi-arm sequence"))).await??;
    within(client.enable("left")).await??;
    let action = |pan, gripper_opening| ActionSequenceArmAction::Move {
        waypoints: vec![],
        goal: ActionSequenceGoal::Joints {
            joints: vec![pan, -0.5, 0.5, 0.0, 0.0],
            max_joint_speed: None,
        },
        gripper_opening,
        max_joint_speed: None,
    };
    let first = within(lease.run_action_sequence(ActionSequenceRequest {
        steps: vec![ActionSequenceStep {
            arms: BTreeMap::from([
                ("left".into(), action(0.1, None)),
                ("right".into(), action(-0.1, None)),
            ]),
        }],
        expected_revisions: BTreeMap::new(),
    }))
    .await??;
    assert!(matches!(first.phase, ActionSequencePhase::Stopped { .. }));
    let state = client.state().await?;
    let left = state
        .arms
        .iter()
        .find(|arm| arm.arm == "left")
        .ok_or("missing left arm")?;
    assert!((left.measured_joints[0] - REST[0]).abs() < 1e-6);
    assert_eq!(left.gripper_opening, Some(0.0));
    within(client.enable("right")).await??;
    let result = within(lease.run_action_sequence(ActionSequenceRequest {
        steps: vec![
            ActionSequenceStep {
                arms: BTreeMap::from([
                    ("left".into(), action(0.1, None)),
                    ("right".into(), action(-0.1, None)),
                ]),
            },
            ActionSequenceStep::single("left", action(0.0, Some(0.2))),
        ],
        expected_revisions: BTreeMap::new(),
    }))
    .await??;
    assert_eq!(result.phase, ActionSequencePhase::Completed);
    assert_eq!(result.arms, ["left", "right"]);
    assert_eq!(
        result.steps.iter().map(Vec::len).collect::<Vec<_>>(),
        [2, 2]
    );
    assert!(
        result.steps[0]
            .iter()
            .all(|record| record.record.outcome() == Some(CommandOutcome::Completed))
    );
    let state = within(client.wait_for_state(|state| {
        state.arms.iter().all(|arm| {
            let target = if arm.arm == "left" { 0.0 } else { -0.1 };
            (arm.measured_joints[0] - target).abs() < 0.02
        })
    }))
    .await??;
    assert_eq!(state.arms.len(), 2);
    Ok(())
}

fn sequence_camera_routes() -> axum::Router {
    axum::Router::new().route(
        "/cameras/{name}",
        axum::routing::get(
            |axum::extract::Path(name): axum::extract::Path<String>| async move {
                if name == "front" {
                    (
                        axum::http::StatusCode::OK,
                        [(axum::http::header::CONTENT_TYPE, "image/jpeg")],
                        "jpeg",
                    )
                } else {
                    (
                        axum::http::StatusCode::NOT_FOUND,
                        [(axum::http::header::CONTENT_TYPE, "image/jpeg")],
                        "",
                    )
                }
            },
        ),
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn model_sequences_retain_camera_evidence_without_recording() -> TestResult {
    let media = sequence_camera_routes();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let media_address = listener.local_addr()?;
    let media_server = format!("http://{media_address}");
    let station = TestStation::start_with_media(&[ARM], None, Some(media_address)).await?;
    let media_task = tokio::spawn(async move { axum::serve(listener, media).await });
    let (client, _) = Station::connect_with(ConnectOptions {
        server: station.url.clone(),
        media_server,
        run_after: None,
    })
    .await?;
    let lease = client.acquire_lease(agent("sequence images")).await?;
    client.enable(ARM).await?;
    let tools = RobotTools::new(&lease, ARM).with_cameras(vec!["front".into(), "missing".into()]);
    let observe_call = |identifier: &str, arguments| RobotToolCall {
        id: identifier.into(),
        name: "observe".into(),
        arguments,
    };
    let missing = tools
        .execute(
            observe_call("missing-history", json!({"source": "last_sequence"})),
            CommandOptions::default(),
        )
        .await?;
    assert!(matches!(missing.outcome, RobotToolOutcome::InvalidInput(_)));
    assert!(missing.cameras.is_empty());
    let result = within(tools.execute(
        RobotToolCall {
            id: "grip".into(),
            name: "action_sequence".into(),
            arguments: json!({"steps": [{"gripper_opening": 1.0}, {"gripper_opening": 0.0}]}),
        },
        CommandOptions::default(),
    ))
    .await??;
    assert!(matches!(result.outcome, RobotToolOutcome::Returned(_)));
    let history = tools.sequence_images().ok_or("missing visual history")?;
    assert_eq!(history.call_id, "grip");
    assert!(history.missed_samples > 0);
    assert!(
        history
            .images
            .iter()
            .all(|image| image.camera == "front" && image.jpeg.as_ref() == b"jpeg")
    );
    assert!(
        history
            .images
            .iter()
            .any(|image| image.boundary == hebbian_rua::SequenceImageBoundary::End)
    );
    assert!(result.cameras.is_empty());
    let current = tools
        .clone()
        .with_cameras(vec!["front".into()])
        .execute(
            observe_call("current", json!({})),
            CommandOptions::default(),
        )
        .await?;
    assert!(current.cameras.contains_key("front"));
    assert!(
        current
            .cameras
            .keys()
            .all(|name| !name.starts_with("Sequence "))
    );
    let review = tools
        .execute(
            observe_call("review", json!({"source": "last_sequence"})),
            CommandOptions::default(),
        )
        .await?;
    assert_eq!(review.data()["source"], "last_sequence");
    assert_eq!(review.data()["sequence_call_id"], "grip");
    assert!(!review.cameras.is_empty());
    assert!(
        review
            .cameras
            .keys()
            .all(|name| name.starts_with("Sequence grip"))
    );
    lease.return_lease().await?;
    media_task.abort();
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn continuous_move_opens_the_jaw_and_finishes_at_the_goal() -> TestResult {
    let station = TestStation::start().await?;
    let client = station.connect().await?;
    let lease = within(client.acquire_lease(agent("waypoints"))).await??;
    within(client.enable(ARM)).await??;
    let mut waypoint = REST.to_vec();
    waypoint[0] += 0.1;
    let mut goal = REST.to_vec();
    goal[0] += 0.23;
    let execution = within(lease.start_action_sequence(ActionSequenceRequest {
        steps: vec![ActionSequenceStep::single(
            ARM,
            ActionSequenceArmAction::Move {
                goal: ActionSequenceGoal::Joints {
                    joints: goal.clone(),
                    max_joint_speed: None,
                },
                waypoints: vec![hebbian_contracts::ActionSequenceWaypoint {
                    at: hebbian_contracts::CompiledGoal::Joints { joints: waypoint },
                    gripper_opening: Some(0.35.try_into()?),
                }],
                gripper_opening: None,
                max_joint_speed: None,
            },
        )],
        expected_revisions: BTreeMap::new(),
    }))
    .await??;
    let before_waypoint = within(
        client.wait_for_state(|state| (0.02..0.08).contains(&state.arms[0].measured_joints[0])),
    )
    .await??;
    assert!(
        before_waypoint.arms[0]
            .gripper_opening
            .is_some_and(|opening| opening < 0.01)
    );
    within(client.wait_for_state(|state| {
        let arm = &state.arms[0];
        (0.1..0.22).contains(&arm.measured_joints[0])
            && arm.gripper_opening.is_some_and(|opening| opening > 0.01)
    }))
    .await??;
    let result = within(client.wait_for_action_sequence(&execution)).await??;
    assert_eq!(result.phase, ActionSequencePhase::Completed);
    assert_eq!(
        result.steps[0][0].record.destination_joints(),
        Some(goal.as_slice())
    );
    let state = within(client.get_state()).await??;
    assert!(
        state.arms[0]
            .gripper_opening
            .is_some_and(|opening| (opening - 0.35).abs() < 0.05)
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn cancelling_before_a_waypoint_leaves_the_gripper_closed() -> TestResult {
    let station = TestStation::start().await?;
    let client = station.connect().await?;
    let lease = within(client.acquire_lease(agent("cancel before release"))).await??;
    within(client.enable(ARM)).await??;
    let source: ActionSequenceRequest = serde_json::from_value(json!({"steps":[{"arms":{(ARM):{
        "goal":{"kind":"joints","joints":[0.4,-0.5,0.5,0.0,0.0]},
        "waypoints":[{"at":{"kind":"joints","joints":[0.22,-0.5,0.5,0.0,0.0]},"gripper_opening":1.0}]
    }}}]}))?;
    let execution = within(lease.start_action_sequence(source)).await??;
    within(client.wait_for_state(|state| (0.01..0.08).contains(&state.arms[0].measured_joints[0])))
        .await??;
    within(client.cancel_action_sequence(&execution)).await??;
    let result = within(client.wait_for_action_sequence(&execution)).await??;
    assert!(matches!(result.phase, ActionSequencePhase::Stopped { .. }));
    let state = within(client.get_state()).await??;
    assert!(state.arms[0].measured_joints[0] < 0.22);
    assert!(
        state.arms[0]
            .gripper_opening
            .is_some_and(|opening| opening < 0.01)
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_waypoint_can_reverse_an_unfinished_step_gripper_move() -> TestResult {
    let station = TestStation::start().await?;
    let client = station.connect().await?;
    let lease = within(client.acquire_lease(agent("reverse jaw during move"))).await??;
    within(client.enable(ARM)).await??;
    let source = serde_json::from_value(json!({"steps":[{"arms":{(ARM):{
        "goal":{"kind":"joints","joints":[0.02,-0.5,0.5,0.0,0.0]},
        "gripper_opening":1.0,
        "max_joint_speed":0.15,
        "waypoints":[{"at":{"kind":"joints","joints":[0.01,-0.5,0.5,0.0,0.0]},"gripper_opening":0.0}]
    }}}]}))?;
    let execution = within(lease.start_action_sequence(source)).await??;
    let result = within(client.wait_for_action_sequence(&execution)).await??;
    assert_eq!(result.phase, ActionSequencePhase::Completed);
    assert_eq!(result.steps[0].len(), 2);
    assert!(
        result.steps[0]
            .iter()
            .all(|entry| entry.record.outcome()
                == Some(hebbian_contracts::CommandOutcome::Completed))
    );
    let state = within(client.get_state()).await??;
    assert!(
        state.arms[0]
            .gripper_opening
            .is_some_and(|opening| opening < 0.02)
    );
    Ok(())
}

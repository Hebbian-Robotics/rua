mod commands;
mod output;

use std::path::Path;
use std::process::ExitCode;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use clap::Parser;
use hebbian_rua::contracts::{
    ActionSequencePhase, ActionSequenceRecord, ActionSequenceRequest, CompilationArtifact,
    CompilationOutcome, JointSpeed, LeaseHolder, MoveArmsCommand, ObservationAnswer,
    ObservationRequest, PhoneInviteRequest, PreviewPose, ProgramContinuation, SnapshotName,
};
use hebbian_rua::{CommandOptions, ConnectOptions, Error, LeaseGuard, LeaseRequest, Station};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use commands::{
    ActionSequenceCommand, CameraCommand, Cli, Command, CompilationCommand, LeaseCommand,
    MotionFileArgs, RecordCommand, TracesCommand, TrajectoryArgs,
};
use output::{Failure, json_value, print_compactable};

// WHY: Release even when the operation fails; preserve the original operation error.
macro_rules! with_motion_lease {
    ($station:expr, $session:expr, $lease:ident, $operation:expr) => {{
        let $lease = motion_lease($station, $session).await?;
        let result: Result<_, Failure> = async { $operation }.await;
        let release_result = $lease.return_lease().await;
        match result {
            Ok(value) => {
                release_result?;
                Ok(value)
            }
            Err(error) => Err(error),
        }
    }};
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let raw = cli.raw
        || matches!(
            cli.command,
            Command::Compilation(
                CompilationCommand::Compile { .. }
                    | CompilationCommand::Inspect { .. }
                    | CompilationCommand::Import { .. }
            )
        );
    let invite_url_only = matches!(cli.command, Command::PhoneInvite { .. }) && !raw;
    match execute(cli).await {
        Ok(value) => {
            if invite_url_only && let Some(url) = value.get("url").and_then(Value::as_str) {
                println!("{url}");
            } else {
                println!("{}", print_compactable(value, raw));
            }
            ExitCode::SUCCESS
        }
        Err(failure) => {
            eprintln!("{}", print_compactable(failure.to_json(), raw));
            failure.exit_code()
        }
    }
}

async fn execute(cli: Cli) -> Result<Value, Failure> {
    let station = Station::connect_with(ConnectOptions {
        server: cli.server,
        media_server: cli.media_server.unwrap_or_default(),
        run_after: None,
    })
    .await?
    .0;
    let session = Session {
        options: CommandOptions {
            operation_id: None,
            trace_context: None,
            expected_revisions: cli.expected_revision.into_iter().collect(),
            ..CommandOptions::default()
        },
        take_lease: cli.take_lease,
    };
    match dispatch(&station, &session, cli.command).await {
        Err(Failure::Sdk {
            error: error @ Error::Refused { .. },
            ..
        }) => {
            // WHY: A refusal for the lease names the holder; looking it up is best effort.
            let holder = station
                .get_state()
                .await
                .ok()
                .and_then(|state| state.lease)
                .map(Box::new);
            Err(Failure::Sdk { error, holder })
        }
        result => result,
    }
}

struct Session {
    options: CommandOptions,
    take_lease: bool,
}

async fn dispatch(
    station: &Station,
    session: &Session,
    command: Command,
) -> Result<Value, Failure> {
    let options = || session.options.clone();
    match command {
        Command::State => json_value(&station.get_state().await?),
        Command::Observation {
            arm,
            camera,
            optional_camera,
            output,
        } => capture_observation(station, arm, camera, optional_camera, &output).await,
        Command::Operation { id } => json_value(&station.get_operation(&id).await?),
        Command::Compilation(command) => compilation(station, session, command).await,
        Command::Enable { arm } => json_value(&station.enable_with(&arm, options()).await?),
        Command::ApproveRecovery { arm } => {
            json_value(&station.approve_recovery_with(&arm, options()).await?)
        }
        Command::Disable { arm } => json_value(&station.disable_with(&arm, options()).await?),
        Command::Stop { arm } => json_value(&station.stop(&arm).await?),
        Command::ClearFault { arm } => {
            json_value(&station.clear_fault_with(&arm, options()).await?)
        }
        Command::Rehome { arm } => json_value(&with_motion_lease!(station, session, lease, {
            Ok(lease.rehome_with(&arm, options()).await?)
        })?),
        Command::MoveJoints {
            arm,
            joints,
            trajectory,
        } => {
            let move_options = motion_options(session, &trajectory);
            let speed = requested_speed(trajectory.speed_radians())?;
            json_value(&with_motion_lease!(station, session, lease, {
                Ok(lease
                    .move_to_joints_with(&arm, joints.into_radians(), speed, move_options)
                    .await?)
            })?)
        }
        Command::MovePose {
            arm,
            pose,
            trajectory,
        } => {
            let move_options = motion_options(session, &trajectory);
            let speed = requested_speed(trajectory.speed_radians())?;
            json_value(&with_motion_lease!(station, session, lease, {
                Ok(lease
                    .move_to_pose_keeping_orientation_with(
                        &arm,
                        pose.position_metres(),
                        pose.orientation(),
                        speed,
                        move_options,
                    )
                    .await?)
            })?)
        }
        Command::MoveArms { input } => move_arms(station, session, &input).await,
        Command::Gripper { arm, opening } => {
            let opening = hebbian_rua::contracts::GripperOpening::try_from(opening)
                .map_err(|error| Failure::Usage(error.to_string()))?;
            json_value(&with_motion_lease!(station, session, lease, {
                Ok(lease
                    .set_gripper_with(&arm, opening, None, options())
                    .await?)
            })?)
        }
        Command::PreviewPose {
            arm,
            pose,
            trajectory,
        } => {
            let mut preview = pose.preview();
            preview.max_joint_speed = requested_speed(trajectory.speed_radians())?;
            preview.max_joint_acceleration = trajectory.acceleration_radians();
            preview.max_joint_jerk = trajectory.jerk_radians();
            json_value(&station.preview_pose(&arm, preview).await?)
        }
        Command::PreviewPoses { arm, input } => {
            let candidates: Vec<PreviewCandidateInput> = read_json(&input)?;
            let candidates = candidates
                .into_iter()
                .map(|candidate| (candidate.label, candidate.pose))
                .collect();
            json_value(&station.preview_pose_candidates(&arm, candidates).await?)
        }
        Command::ActionSequence(command) => action_sequence(station, session, command).await,
        Command::Lease(LeaseCommand::Status) => json_value(&station.lease_status().await?),
        Command::Lease(LeaseCommand::Revoke) => {
            station.revoke_lease().await?;
            json_value(&json!({"revoked": true}))
        }
        Command::Record(command) => record(station, command).await,
        Command::PhoneInvite { label } => {
            json_value(&station.phone_invite(&PhoneInviteRequest { label }).await?)
        }
        Command::Camera(command) => camera(station, command).await,
        Command::Traces(command) => traces(station, command).await,
    }
}

async fn move_arms(
    station: &Station,
    session: &Session,
    input: &MotionFileArgs,
) -> Result<Value, Failure> {
    let command: MoveArmsCommand = read_json(input)?;
    json_value(&with_motion_lease!(station, session, lease, {
        Ok(lease
            .run_arms_with(command, session.options.clone())
            .await?)
    })?)
}

async fn capture_observation(
    station: &Station,
    arm: Vec<String>,
    camera: Vec<String>,
    optional_camera: Vec<String>,
    output: &Path,
) -> Result<Value, Failure> {
    let answer = station
        .capture_observation_http(&ObservationRequest {
            arms: arm,
            required_cameras: parse_camera_names(camera)?,
            optional_cameras: parse_camera_names(optional_camera)?,
            wait: None,
        })
        .await?;
    match answer {
        ObservationAnswer::ObservationCaptured {
            state,
            cameras,
            missing_optional_cameras,
        } => {
            if !cameras.is_empty() {
                std::fs::create_dir_all(output).map_err(|error| Failure::Io(error.to_string()))?;
            }
            let mut frames = serde_json::Map::new();
            for (name, frame) in cameras {
                SnapshotName::try_from(name.clone()).map_err(|error| {
                    Failure::Io(format!("invalid camera name from station: {error}"))
                })?;
                let bytes = STANDARD
                    .decode(&frame.jpeg_base64)
                    .map_err(|error| Failure::Io(format!("invalid camera frame: {error}")))?;
                let path = output.join(format!("{name}.jpg"));
                write_file(&path, &bytes)?;
                frames.insert(name, json!({"path": path, "captured_at_unix_ns": frame.captured_at_unix_ns, "retrieved_between_unix_seconds": frame.retrieved_between_unix_seconds}));
            }
            Ok(
                json!({"status": "observation_captured", "state": state, "cameras": frames, "missing_optional_cameras": missing_optional_cameras}),
            )
        }
        other => Err(Failure::Rejected {
            kind: "observation_failed".into(),
            detail: "station could not capture the observation".into(),
            record: json!(other),
        }),
    }
}

fn parse_camera_names(names: Vec<String>) -> Result<Vec<SnapshotName>, Failure> {
    names
        .into_iter()
        .map(|name| SnapshotName::try_from(name).map_err(|error| Failure::Usage(error.to_string())))
        .collect()
}

async fn compilation(
    station: &Station,
    session: &Session,
    command: CompilationCommand,
) -> Result<Value, Failure> {
    match command {
        CompilationCommand::Compile { input } => {
            compilation_outcome(station.compile_action_sequence(&read_json(&input)?).await?)
        }
        CompilationCommand::Import { path } => {
            let text = std::fs::read_to_string(&path)
                .map_err(|error| Failure::Io(format!("{}: {error}", path.display())))?;
            let artifact: CompilationArtifact = serde_json::from_str(&text)
                .map_err(|error| Failure::Usage(format!("{}: {error}", path.display())))?;
            compilation_outcome(station.import_compilation(&artifact).await?)
        }
        CompilationCommand::Inspect { id } => json_value(&station.inspect_compilation(&id).await?),
        CompilationCommand::Run {
            id,
            prior_execution,
            next_step,
        } => {
            let continuation = match (prior_execution, next_step) {
                (Some(prior_execution_id), Some(next_step)) => Some(ProgramContinuation {
                    prior_execution_id,
                    next_step,
                }),
                (None, None) => None,
                _ => {
                    return Err(Failure::Usage(
                        "both --prior-execution and --next-step are required".into(),
                    ));
                }
            };
            let record = with_motion_lease!(station, session, lease, {
                let record = lease
                    .start_compiled_execution(
                        &id,
                        session.options.expected_revisions.clone(),
                        continuation,
                    )
                    .await?
                    .wait()
                    .await?;
                finished_action_sequence(&record)?;
                Ok(record)
            })?;
            Ok(json!({"execution_id": record.id, "result": record}))
        }
    }
}

fn compilation_outcome(outcome: CompilationOutcome) -> Result<Value, Failure> {
    match outcome {
        CompilationOutcome::Compiled { artifact } => json_value(&*artifact),
        rejected @ CompilationOutcome::Rejected { .. } => Err(Failure::Rejected {
            kind: "compilation_rejected".into(),
            detail: "station rejected the compilation".into(),
            record: json!(rejected),
        }),
    }
}

fn requested_speed(speed: Option<f64>) -> Result<Option<JointSpeed>, Failure> {
    speed
        .map(JointSpeed::new)
        .transpose()
        .map_err(|error| Failure::Usage(error.to_string()))
}

fn motion_options(session: &Session, trajectory: &TrajectoryArgs) -> CommandOptions {
    CommandOptions {
        max_joint_acceleration: trajectory.acceleration_radians(),
        max_joint_jerk: trajectory.jerk_radians(),
        ..session.options.clone()
    }
}

async fn motion_lease(station: &Station, session: &Session) -> Result<LeaseGuard, Failure> {
    let request = LeaseRequest::new(LeaseHolder::Agent, "hebbian-rua");
    Ok(if session.take_lease {
        station.take_lease(request).await?
    } else {
        station.acquire_lease(request).await?
    })
}

#[derive(serde::Deserialize)]
struct PreviewCandidateInput {
    #[serde(default)]
    label: Option<String>,
    #[serde(flatten)]
    pose: PreviewPose,
}

async fn action_sequence(
    station: &Station,
    session: &Session,
    command: ActionSequenceCommand,
) -> Result<Value, Failure> {
    match command {
        ActionSequenceCommand::Run { input, camera } => {
            let mut request: ActionSequenceRequest = read_json(&input)?;
            for (arm, revision) in &session.options.expected_revisions {
                if request
                    .expected_revisions
                    .get(arm)
                    .is_some_and(|existing| existing != revision)
                {
                    return Err(Failure::Usage(format!(
                        "conflicting expected revisions for arm {arm}"
                    )));
                }
                request.expected_revisions.insert(arm.clone(), *revision);
            }
            let cameras = parse_camera_names(camera)?;
            let capture_requested = !cameras.is_empty();
            let record = with_motion_lease!(station, session, lease, {
                let record = lease
                    .start_sequence_execution(request, cameras)
                    .await?
                    .wait()
                    .await?;
                finished_action_sequence(&record)?;
                Ok(record)
            })?;
            if capture_requested {
                Ok(json!({"execution_id": record.id, "result": record}))
            } else {
                json_value(&record)
            }
        }
        ActionSequenceCommand::Cancel { id } => {
            let mut record = station.get_action_sequence(&id).await?;
            if matches!(record.phase, ActionSequencePhase::Running { .. }) {
                station.cancel_action_sequence(&id).await?;
                record = station.wait_for_action_sequence(&id).await?;
            }
            json_value(&record)
        }
        ActionSequenceCommand::Get { id } => json_value(&station.get_action_sequence(&id).await?),
        ActionSequenceCommand::Images { id, output } => {
            sequence_images(station, &id, &output).await
        }
    }
}

async fn sequence_images(station: &Station, id: &str, output: &Path) -> Result<Value, Failure> {
    let Some(history) = station.get_sequence_camera_history(id).await? else {
        return Ok(Value::Null);
    };
    if !history.images.is_empty() {
        std::fs::create_dir_all(output).map_err(|error| Failure::Io(error.to_string()))?;
    }
    let mut images = Vec::with_capacity(history.images.len());
    for (index, image) in history.images.into_iter().enumerate() {
        let bytes = STANDARD
            .decode(&image.frame.jpeg_base64)
            .map_err(|error| Failure::Io(format!("invalid camera frame: {error}")))?;
        let path = output.join(format!("{index:03}.jpg"));
        write_file(&path, &bytes)?;
        images.push(json!({"camera": image.camera, "path": path, "elapsed_seconds": image.elapsed_seconds, "boundary": image.boundary, "captured_at_unix_ns": image.frame.captured_at_unix_ns, "retrieved_between_unix_seconds": image.frame.retrieved_between_unix_seconds}));
    }
    Ok(
        json!({"expires_in_seconds": history.expires_in_seconds, "missed_samples": history.missed_samples, "truncated": history.truncated, "images": images}),
    )
}

fn finished_action_sequence(record: &ActionSequenceRecord) -> Result<(), Failure> {
    match &record.phase {
        ActionSequencePhase::Stopped { failure } => Err(Failure::ActionSequenceStopped {
            detail: failure.detail().to_owned(),
            record: json!(record),
        }),
        ActionSequencePhase::Running { .. } | ActionSequencePhase::Completed => Ok(()),
    }
}

async fn record(station: &Station, command: RecordCommand) -> Result<Value, Failure> {
    match command {
        RecordCommand::Start => json_value(&json!({"name": station.start_recording().await?})),
        RecordCommand::Stop => {
            station.stop_recording().await?;
            json_value(&json!({"stopped": true}))
        }
        RecordCommand::Status => json_value(&station.recording_status().await?),
    }
}

async fn camera(station: &Station, command: CameraCommand) -> Result<Value, Failure> {
    match command {
        CameraCommand::List => json_value(&station.list_cameras().await?),
        CameraCommand::Snapshot { name, out } => {
            let frame = station.camera_jpeg(&name).await?;
            write_file(&out, &frame.jpeg)?;
            json_value(&json!({
                "path": out,
                "bytes": frame.jpeg.len(),
                "captured_at_unix_ns": frame.captured_at_unix_ns,
            }))
        }
    }
}

async fn traces(station: &Station, command: TracesCommand) -> Result<Value, Failure> {
    match command {
        TracesCommand::List => json_value(&station.list_traces().await?),
        TracesCommand::Pull { names, output } => {
            let listed = station.list_traces().await?;
            let wanted: Vec<String> = if names.is_empty() {
                listed.into_iter().map(|trace| trace.name).collect()
            } else {
                names
            };
            std::fs::create_dir_all(&output).map_err(|error| Failure::Io(error.to_string()))?;
            let mut written = Vec::new();
            for name in wanted {
                let bytes = station.download_trace(&name).await?;
                // INVARIANT: The name comes from the station; never let it leave the directory.
                let file_name = Path::new(&name)
                    .file_name()
                    .ok_or_else(|| Failure::Usage(format!("{name} is not a file name")))?;
                let destination = output.join(file_name);
                write_file(&destination, &bytes)?;
                written.push(destination);
            }
            json_value(&written)
        }
    }
}

fn write_file(path: &Path, bytes: &[u8]) -> Result<(), Failure> {
    std::fs::write(path, bytes).map_err(|error| Failure::Io(format!("{}: {error}", path.display())))
}

fn read_json<T: DeserializeOwned>(request: &MotionFileArgs) -> Result<T, Failure> {
    let input = request.path.as_path();
    let text = if input == Path::new("-") {
        std::io::read_to_string(std::io::stdin())
    } else {
        std::fs::read_to_string(input)
    }
    .map_err(|error| Failure::Io(format!("{}: {error}", input.display())))?;
    let usage =
        |error: &dyn std::fmt::Display| Failure::Usage(format!("{}: {error}", input.display()));
    let arguments = serde_json::from_str(&text).map_err(|error| usage(&error))?;
    let arguments = if request.input_station {
        arguments
    } else {
        hebbian_rua::normalize_robot_tool_arguments(arguments).map_err(|error| usage(&error))?
    };
    serde_json::from_value(arguments).map_err(|error| usage(&error))
}

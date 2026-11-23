mod commands;
mod output;

use std::path::Path;
use std::process::ExitCode;

use clap::Parser;
use hebbian_rua::contracts::{
    ActionSequencePhase, ActionSequenceRecord, ActionSequenceRequest, LeaseHolder, MoveArmsCommand,
    PhoneInviteRequest, PreviewPose,
};
use hebbian_rua::{CommandOptions, ConnectOptions, Error, LeaseGuard, LeaseRequest, Station};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use commands::{
    ActionSequenceCommand, CameraCommand, Cli, Command, LeaseCommand, RecordCommand, TracesCommand,
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
    let raw = cli.raw;
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
        Command::MoveJoints { arm, joints, speed } => {
            json_value(&with_motion_lease!(station, session, lease, {
                Ok(lease
                    .move_to_joints_with(&arm, joints, speed, options())
                    .await?)
            })?)
        }
        Command::MovePose { arm, pose, speed } => {
            json_value(&with_motion_lease!(station, session, lease, {
                Ok(lease
                    .move_to_pose_keeping_orientation_with(
                        &arm,
                        pose.position(),
                        pose.orientation(),
                        speed,
                        options(),
                    )
                    .await?)
            })?)
        }
        Command::MoveArms { input } => {
            let command: MoveArmsCommand = read_json(&input)?;
            json_value(&with_motion_lease!(station, session, lease, {
                Ok(lease.run_arms_with(command, options()).await?)
            })?)
        }
        Command::Gripper { arm, opening } => {
            let opening = hebbian_rua::contracts::GripperOpening::try_from(opening)
                .map_err(|error| Failure::Usage(error.to_string()))?;
            json_value(&with_motion_lease!(station, session, lease, {
                Ok(lease
                    .set_gripper_with(&arm, opening, None, options())
                    .await?)
            })?)
        }
        Command::PreviewPose { arm, pose } => {
            json_value(&station.preview_pose(&arm, pose.preview()).await?)
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

async fn motion_lease(station: &Station, session: &Session) -> Result<LeaseGuard, Failure> {
    let request = LeaseRequest::new(LeaseHolder::Agent, "hebbian-robot-use");
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
        ActionSequenceCommand::Run { input } => {
            let request: ActionSequenceRequest = read_json(&input)?;
            let record = with_motion_lease!(station, session, lease, {
                let record = lease.run_action_sequence(request).await?;
                finished_action_sequence(&record)?;
                Ok(record)
            })?;
            json_value(&record)
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
    }
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

fn read_json<T: DeserializeOwned>(input: &Path) -> Result<T, Failure> {
    let text = if input == Path::new("-") {
        std::io::read_to_string(std::io::stdin())
    } else {
        std::fs::read_to_string(input)
    }
    .map_err(|error| Failure::Io(format!("{}: {error}", input.display())))?;
    serde_json::from_str(&text)
        .map_err(|error| Failure::Usage(format!("{}: {error}", input.display())))
}

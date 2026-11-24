mod camera_lifecycle;
mod camera_worker;
mod config;
mod snapshots;
mod usb_camera;

use livekit_cameras::error;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "hebbian-rua-media")]
struct MediaArguments {
    #[command(subcommand)]
    command: MediaCommand,
}

#[derive(Subcommand)]
enum MediaCommand {
    /// List local video capture devices.
    CameraDevices,
    #[command(hide = true)]
    ProbeLivekit,
    Record(livekit_cameras::recorder::Arguments),
    Serve(config::ServeArguments),
}

use config::ServeArguments;
use error::PublisherError;
use hebbian_contracts::{RECORDER_CONTROLLER_INTERRUPTED_EXIT_CODE, RecordingWarning};
use livekit_camera_publisher::capture;
use station_access::Credentials;
use std::{io, process::ExitCode, sync::Arc};
use tokio_util::sync::CancellationToken;

async fn serve(arguments: ServeArguments) -> Result<(), PublisherError> {
    // WHY: Capture subprocesses handle native permissions so HTTP can start while
    // macOS is still waiting for camera access.
    let cameras = arguments.cameras()?;
    let mut termination =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let listener = tokio::net::TcpListener::bind(arguments.address).await?;
    let stopped = CancellationToken::new();
    let mut publishing_tasks = tokio::task::JoinSet::new();
    let room = match &arguments.room {
        Some(room) if !cameras.is_empty() => Some(Arc::new(camera_worker::RoomPublication {
            room: room.clone(),
            credentials: Credentials::from_environment()?,
        })),
        _ => None,
    };
    if room.is_some() {
        let telemetry_stopped = stopped.clone();
        publishing_tasks.spawn(async move {
            if let Err(error) =
                livekit_cameras::follow_station(&arguments.station_url, telemetry_stopped).await
            {
                tracing::warn!(%error, "Station telemetry stopped");
            }
        });
    }
    let snapshots = snapshots::Snapshots::start(
        cameras
            .iter()
            .map(|camera| hebbian_contracts::CameraName::try_from(camera.name()))
            .collect::<Result<_, _>>()?,
        arguments.camera_socket,
        arguments.room,
    );
    for camera in cameras {
        publishing_tasks.spawn(camera_worker::supervise(
            camera,
            room.clone(),
            snapshots.clone(),
            stopped.clone(),
        ));
    }
    let shutdown = stopped.clone();
    let result = axum::serve(listener, snapshots.router())
        .with_graceful_shutdown(async move {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {},
                _ = termination.recv() => {},
                () = livekit_cameras::parent_process::wait_for_station_exit(arguments.station_pid) => {},
            }
            shutdown.cancel();
        })
        .await;
    stopped.cancel();
    while let Some(result) = publishing_tasks.join_next().await {
        if let Err(error) = result {
            tracing::error!(%error, "Camera publishing task failed");
        }
    }
    result?;
    Ok(())
}

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                tracing_subscriber::EnvFilter::new("warn,livekit_cameras=info")
            }),
        )
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();
    let result = if std::env::args_os()
        .nth(1)
        .is_some_and(|argument| argument == "__capture")
    {
        capture::run_capture_process()
            .map(|()| ExitCode::SUCCESS)
            .map_err(PublisherError::from)
    } else {
        run_with_runtime()
    };
    match result {
        Ok(exit_code) => exit_code,
        Err(error) => {
            eprintln!("hebbian-rua-media: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run_with_runtime() -> Result<ExitCode, PublisherError> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run())
}

async fn run() -> Result<ExitCode, PublisherError> {
    match MediaArguments::parse().command {
        MediaCommand::CameraDevices => list_camera_devices()?,
        MediaCommand::ProbeLivekit => probe_livekit().await?,
        MediaCommand::Serve(arguments) => serve(arguments).await?,
        MediaCommand::Record(arguments) => {
            match livekit_cameras::recorder::record(arguments).await? {
                None => {}
                Some(RecordingWarning::ControllerInterrupted) => {
                    tracing::warn!("Recording finalized with a gap in arm telemetry");
                    return Ok(ExitCode::from(RECORDER_CONTROLLER_INTERRUPTED_EXIT_CODE));
                }
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn list_camera_devices() -> Result<(), PublisherError> {
    cameras::initialize()?;
    let devices: Vec<_> = cameras::list_cameras()?
        .into_iter()
        .map(|camera| {
            serde_json::json!({
                "name": camera.human_name(),
                "device": camera.index().as_string(),
            })
        })
        .collect();
    println!("{}", serde_json::Value::Array(devices));
    Ok(())
}

async fn probe_livekit() -> Result<(), PublisherError> {
    use hebbian_contracts::RoomAccess;
    use livekit::{Room, RoomOptions, track::TrackKind};

    // INVARIANT: Read the short-lived room token from stdin to keep it out of process arguments.
    let access: RoomAccess = serde_json::from_reader(io::stdin().lock())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "Invalid room access input"))?;
    let mut options = RoomOptions::default();
    options.auto_subscribe = false;
    let (room, _events) = Room::connect(&access.server_url, &access.token, options).await?;
    let has_video = room.remote_participants().values().any(|participant| {
        participant
            .track_publications()
            .values()
            .any(|publication| publication.kind() == TrackKind::Video && !publication.is_muted())
    });
    room.close().await?;
    println!("{has_video}");
    Ok(())
}

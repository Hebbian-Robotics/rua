mod config;
mod snapshots;
mod usb_camera;

use livekit_cameras::error;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "hebbian-robot-use-media")]
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

use config::{CameraInput, ServeArguments};
use error::PublisherError;
use livekit_camera_publisher::{capture, publisher};
use station_access::Credentials;
use std::{io, sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;

async fn publish_camera_until_stopped(
    camera: CameraInput,
    room: String,
    credentials: Arc<Credentials>,
    stopped: CancellationToken,
) {
    const RETRY_DELAY: Duration = Duration::from_secs(3);
    while !stopped.is_cancelled() {
        let result = async {
            let access = credentials.camera_access(
                &room,
                camera.name(),
                station_access::CameraLifetime::Station,
            )?;
            let publishing_camera = camera.resolve().await?;
            publisher::publish_camera(
                publishing_camera,
                &access.server_url,
                &access.token,
                stopped.clone(),
            )
            .await?;
            Ok::<(), PublisherError>(())
        }
        .await;
        if stopped.is_cancelled() {
            break;
        }
        // PROTOCOL: The station reads per-camera status from unprefixed lines.
        match result {
            Ok(()) => eprintln!("Camera {}: publisher stopped", camera.name()),
            Err(error) => eprintln!("Camera {}: {error}", camera.name()),
        }
        tokio::select! {
            () = stopped.cancelled() => break,
            () = tokio::time::sleep(RETRY_DELAY) => {},
        }
    }
}

async fn serve(arguments: ServeArguments) -> Result<(), PublisherError> {
    // WHY: Capture subprocesses handle native permissions so HTTP can start while
    // macOS is still waiting for camera access.
    let cameras = arguments.cameras()?;
    let mut termination =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let listener = tokio::net::TcpListener::bind(arguments.address).await?;
    let stopped = CancellationToken::new();
    let mut publishing_tasks = tokio::task::JoinSet::new();
    let (local_cameras, expected_cameras) = if let Some(room) = &arguments.room {
        let expected_cameras = cameras
            .iter()
            .map(|camera| camera.name().to_owned())
            .collect();
        if !cameras.is_empty() {
            let credentials = Arc::new(Credentials::from_environment()?);
            for camera in cameras {
                publishing_tasks.spawn(publish_camera_until_stopped(
                    camera,
                    room.clone(),
                    credentials.clone(),
                    stopped.clone(),
                ));
            }
            let telemetry_stopped = stopped.clone();
            publishing_tasks.spawn(async move {
                if let Err(error) =
                    livekit_cameras::follow_station(&arguments.station_url, telemetry_stopped).await
                {
                    tracing::warn!(%error, "Station telemetry stopped");
                }
            });
        }
        // WHY: Snapshots use the room's frames while publishers own the native devices.
        (Vec::new(), expected_cameras)
    } else {
        (cameras, Vec::new())
    };
    let snapshots = snapshots::Snapshots::start(
        local_cameras,
        expected_cameras,
        arguments.camera_socket,
        arguments.room,
    );
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

fn main() -> std::process::ExitCode {
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
        capture::run_capture_process().map_err(PublisherError::from)
    } else {
        run_with_runtime()
    };
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("hebbian-robot-use-media: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn run_with_runtime() -> Result<(), PublisherError> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run())
}

async fn run() -> Result<(), PublisherError> {
    match MediaArguments::parse().command {
        MediaCommand::CameraDevices => list_camera_devices(),
        MediaCommand::ProbeLivekit => probe_livekit().await,
        MediaCommand::Serve(arguments) => serve(arguments).await,
        MediaCommand::Record(arguments) => {
            livekit_cameras::recorder::record(arguments).await?;
            Ok(())
        }
    }
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

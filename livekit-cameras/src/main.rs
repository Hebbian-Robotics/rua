mod config;
mod snapshots;

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
    Record(livekit_cameras::recorder::Arguments),
    Serve(config::ServeArguments),
}

use config::{CameraConfiguration, ServeArguments};
use error::PublisherError;
use livekit_camera_publisher::{capture, publisher};
use station_access::Credentials;
use std::{sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;

async fn publish_camera_until_stopped(
    camera: CameraConfiguration,
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
            let mut publishing_camera =
                CameraConfiguration::new(camera.name().to_owned(), camera.device().to_owned())?;
            publishing_camera.publishing = camera.publishing;
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
            // WHY: Request native camera permission before starting capture subprocesses.
            if let Err(error) = cameras::initialize() {
                tracing::warn!(%error, "Camera initialization unavailable");
            }
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
                () = livekit_cameras::supervisor::exited(arguments.supervisor_pid) => {},
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
        MediaCommand::Serve(arguments) => serve(arguments).await,
        MediaCommand::Record(arguments) => {
            livekit_cameras::recorder::record(arguments).await?;
            Ok(())
        }
    }
}

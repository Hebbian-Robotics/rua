mod config;
mod snapshots;

use livekit_cameras::error;

use clap::{Parser, Subcommand};
use std::{net::SocketAddr, path::PathBuf};

#[derive(Parser)]
#[command(name = "hebbian-robot-use-media")]
struct MediaArguments {
    #[command(subcommand)]
    command: MediaCommand,
}

#[derive(Subcommand)]
enum MediaCommand {
    Publish(Arguments),
    Record(livekit_cameras::recorder::Arguments),
    Serve {
        #[arg(long, default_value = "127.0.0.1:8781")]
        address: SocketAddr,
        /// Native camera mapping (name=device), repeatable.
        #[arg(long = "camera")]
        cameras: Vec<String>,
        /// A camera another program publishes into the room, by name: listed as unavailable
        /// until its frames arrive. Repeatable.
        #[arg(long = "expected-camera")]
        expected_cameras: Vec<String>,
        /// Unix socket exposed by the simulator.
        #[arg(long)]
        camera_socket: Option<PathBuf>,
        /// Subscribe to cameras in this `LiveKit` room.
        #[arg(long)]
        room: Option<String>,
    },
}
use config::Arguments;
use error::PublisherError;
use livekit_camera_publisher::{capture, publisher};
use station_access::Credentials;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

struct CameraExit {
    name: String,
    result: Result<(), PublisherError>,
}

async fn publish(arguments: Arguments) -> Result<(), PublisherError> {
    let cameras = arguments.cameras()?;
    let credentials = Arc::new(Credentials::from_environment()?);
    // WHY: Request native camera permission once, before starting the capture subprocesses.
    cameras::initialize()?;
    let stopped = CancellationToken::new();
    let mut interruption =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
    let mut termination =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let telemetry_stopped = stopped.clone();
    let telemetry = tokio::spawn(async move {
        if let Err(error) =
            livekit_cameras::follow_station(&arguments.station_url, telemetry_stopped).await
        {
            tracing::warn!(%error, "Station telemetry stopped");
        }
    });
    let mut publishers = tokio::task::JoinSet::new();
    for camera in cameras {
        let camera_name = camera.name().to_owned();
        let camera_credentials = credentials.clone();
        let room_name = arguments.room.clone();
        let camera_stopped = stopped.child_token();
        publishers.spawn(async move {
            let result = async {
                let access = camera_credentials.camera_access(
                    &room_name,
                    camera.name(),
                    station_access::CameraLifetime::Station,
                )?;
                publisher::publish_camera(
                    camera,
                    &access.server_url,
                    &access.token,
                    camera_stopped,
                )
                .await?;
                Ok(())
            }
            .await;
            CameraExit {
                name: camera_name,
                result,
            }
        });
    }
    let all_stopped = loop {
        tokio::select! {
            _ = interruption.recv() => break false,
            _ = termination.recv() => break false,
            Some(result) = publishers.join_next(), if !publishers.is_empty() => {
                report_camera_result(result);
                if publishers.is_empty() { break true; }
            }
        }
    };
    stopped.cancel();
    while let Some(result) = publishers.join_next().await {
        report_camera_result(result);
    }
    telemetry.await?;
    if all_stopped {
        Err(PublisherError::NoActiveCameras)
    } else {
        Ok(())
    }
}

fn report_camera_result(result: Result<CameraExit, tokio::task::JoinError>) {
    // PROTOCOL: The station reads per-camera status from the unprefixed lines.
    match result {
        Ok(CameraExit {
            name,
            result: Err(error),
        }) => eprintln!("Camera {name}: {error}"),
        Err(error) => tracing::error!(%error, "Camera publisher task failed"),
        Ok(CameraExit {
            name,
            result: Ok(()),
        }) => eprintln!("Camera {name}: stopped"),
    }
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
        MediaCommand::Publish(arguments) => publish(arguments).await,
        MediaCommand::Record(arguments) => {
            livekit_cameras::recorder::record(arguments).await?;
            Ok(())
        }
        MediaCommand::Serve {
            address,
            cameras,
            expected_cameras,
            camera_socket,
            room,
        } => {
            let snapshots =
                snapshots::Snapshots::start(cameras, expected_cameras, camera_socket, room)?;
            let listener = tokio::net::TcpListener::bind(address).await?;
            axum::serve(listener, snapshots.router())
                .with_graceful_shutdown(async {
                    match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                        Ok(mut termination) => { tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = termination.recv() => {} } },
                        Err(error) => tracing::warn!(%error, "Could not listen for SIGTERM"),
                    }
                }).await?;
            Ok(())
        }
    }
}

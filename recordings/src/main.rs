use axum::{Router, response::Redirect, routing::get};
use clap::Parser;
use recordings::{RecordingLibrary, router};
use std::{net::SocketAddr, path::PathBuf};
use tower_http::{services::ServeDir, set_header::SetResponseHeaderLayer};
#[derive(Parser)]
struct Arguments {
    #[arg(long)]
    directory: PathBuf,
    #[arg(long, default_value = "127.0.0.1:8790")]
    address: SocketAddr,
    #[arg(long)]
    viewer_directory: Option<PathBuf>,
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments = Arguments::parse();
    let viewer = Router::new()
        .route(
            "/",
            get(|| async { Redirect::temporary("/viewer/recording.html") }),
        )
        .fallback_service(ServeDir::new(
            arguments
                .viewer_directory
                .unwrap_or_else(default_viewer_directory),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            axum::http::header::CACHE_CONTROL,
            axum::http::HeaderValue::from_static("no-cache"),
        ));
    let app = Router::new()
        .nest(
            "/viewer/api",
            router(RecordingLibrary::new(arguments.directory)),
        )
        .route(
            "/",
            get(|| async { Redirect::temporary("/viewer/recording.html") }),
        )
        .nest("/viewer/", viewer);
    let listener = tokio::net::TcpListener::bind(arguments.address).await?;
    println!(
        "Recordings: http://{}/viewer/recording.html",
        listener.local_addr()?
    );
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown())
        .await?;
    Ok(())
}
async fn shutdown() {
    #[cfg(unix)]
    {
        if let Ok(mut terminate) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = terminate.recv() => {} }
            return;
        }
    }
    let _ = tokio::signal::ctrl_c().await;
}

fn default_viewer_directory() -> PathBuf {
    let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let requested = repository.join("examples/typescript/viewer/dist");
    if requested.is_dir() {
        requested
    } else {
        repository.join("dist/viewer")
    }
}

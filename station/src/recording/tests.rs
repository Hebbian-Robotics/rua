use super::*;
use tokio::sync::mpsc;

#[tokio::test]
async fn shutdown_waits_for_the_final_room_announcement() -> Result<(), Box<dyn std::error::Error>>
{
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let (requested, mut requests) = mpsc::unbounded_channel();
    let release = Arc::new(tokio::sync::Notify::new());
    let release_request = Arc::clone(&release);
    let routes = Router::new()
        .route(
            "/twirp/livekit.RoomService/CreateRoom",
            post(|| async { Vec::<u8>::new() }),
        )
        .route(
            "/twirp/livekit.RoomService/UpdateRoomMetadata",
            post(move || {
                let requested = requested.clone();
                let release = Arc::clone(&release_request);
                async move {
                    let _ = requested.send(());
                    release.notified().await;
                    Vec::<u8>::new()
                }
            }),
        );
    let server = tokio::spawn(async move { axum::serve(listener, routes).await });
    let directory = tempfile::tempdir()?;
    let manager = RecordingManager::new(
        Some(RecordingConfiguration {
            credentials: station_access::Credentials::from_values([
                Some(format!("http://{address}")),
                Some("key".into()),
                Some("secret".into()),
            ])?,
            room: "test".into(),
            directory: directory.path().to_owned(),
            executable: PathBuf::from("unused"),
        }),
        "http://127.0.0.1:1".into(),
    );
    manager.announce(false);
    tokio::time::timeout(Duration::from_secs(2), requests.recv())
        .await?
        .ok_or("announcement did not arrive")?;
    let mut shutdown = Box::pin(manager.shutdown());
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut shutdown)
            .await
            .is_err()
    );
    assert_eq!(
        manager.start_recording().unwrap_err(),
        "The station is shutting down"
    );
    release.notify_one();
    tokio::time::timeout(Duration::from_secs(2), shutdown).await?;
    server.abort();
    Ok(())
}

//! Exercise the SDK's event-stream timeout against a loopback HTTP server.

use std::convert::Infallible;
use std::time::Duration;

use axum::Router;
use axum::http::HeaderMap;
use axum::response::IntoResponse;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::get;
use futures_util::{StreamExt, stream};
use hebbian_rua::Station;
use tokio::net::TcpListener;
use tokio::sync::mpsc;

type TestResult = Result<(), Box<dyn std::error::Error>>;

async fn open_event_stream(
    keepalive: bool,
) -> Result<(Station, mpsc::UnboundedReceiver<HeaderMap>), Box<dyn std::error::Error>> {
    let (requests, received) = mpsc::unbounded_channel();
    let app = Router::new().route(
        "/events",
        get(move |headers: HeaderMap| {
            let requests = requests.clone();
            async move {
                let resumed = headers.contains_key("last-event-id");
                let _ = requests.send(headers);
                let snapshot = Event::default().id("7").event("snapshot").data(
                    r#"{"station_instance_id":"station","sequence":7,"items":[],"run_sequence":0}"#,
                );
                let events = stream::iter((!resumed).then_some(Ok::<_, Infallible>(snapshot)))
                    .chain(stream::pending());
                let response = Sse::new(events);
                if keepalive {
                    response
                        .keep_alive(KeepAlive::new().interval(Duration::from_secs(5)))
                        .into_response()
                } else {
                    response.into_response()
                }
            }
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let url = format!("http://{}", listener.local_addr()?);
    tokio::spawn(async move { axum::serve(listener, app).await });
    let station = Station::connect(&url).await?;
    station.wait_until_connected().await?;
    Ok((station, received))
}

#[tokio::test]
async fn a_silent_stream_reconnects_with_the_last_event_id() -> TestResult {
    let (_station, mut requests) = open_event_stream(false).await?;
    let first = requests.recv().await.ok_or("missing initial request")?;
    assert!(!first.contains_key("last-event-id"));
    tokio::time::pause();

    let resumed = tokio::time::timeout(Duration::from_secs(35), requests.recv())
        .await?
        .ok_or("missing resume request")?;
    assert_eq!(resumed.get("last-event-id"), Some(&"7".parse()?));
    Ok(())
}

#[tokio::test]
async fn keepalive_comments_keep_an_idle_stream_connected() -> TestResult {
    let (station, mut requests) = open_event_stream(true).await?;
    requests.recv().await.ok_or("missing initial request")?;
    tokio::time::pause();

    tokio::time::sleep(Duration::from_secs(32)).await;
    station.wait_until_connected().await?;
    assert!(matches!(
        requests.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));
    Ok(())
}

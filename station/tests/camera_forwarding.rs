#![allow(
    clippy::unwrap_used,
    reason = "This integration test may abort on setup failure or an unexpected response."
)]

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use axum::routing::get;
use hebbian_contracts::{CameraListing, CommandAnswer, MediaState, StationInstance};
use hebbian_rua_core::config::ControllerConfig;
use hebbian_rua_core::control_link::Link;
use hebbian_rua_core::lease::{LEASE, Lease};
use hebbian_rua_core::plant::NoPhysicsStandIn;
use hebbian_rua_core::station_api::HttpServer;
use hebbian_rua_server::media_status::MediaStatus;
use hebbian_rua_server::web::{self, CameraForwarding, WebSettings};
use reqwest::{Client, StatusCode};
use serde_json::json;
use tokio::sync::Semaphore;

fn station(media_address: SocketAddr, camera_forwarding: CameraForwarding) -> HttpServer {
    let media_status = MediaStatus::default();
    media_status.set(MediaState::Running);
    HttpServer::start(
        "127.0.0.1:0",
        Vec::new(),
        Arc::new(hebbian_rua_server::run_telemetry::LiveRunStore::default()),
        Arc::default(),
        |runtime, address| {
            let (router, _) = web::mount(
                runtime,
                address,
                WebSettings {
                    local_cameras_only: true,
                    recordings_directory: std::env::temp_dir(),
                    enable_recording: false,
                    media_address: Some(media_address),
                    camera_forwarding,
                    media_executable: "unused".into(),
                    room: None,
                    phone_page_url: web::DEFAULT_PHONE_PAGE_URL.into(),
                    media_status,
                },
                web::MediaCapabilities::LocalOnly,
                || true,
            );
            Ok(router)
        },
    )
    .unwrap()
}

#[test]
fn stalled_snapshots_shed_load_and_leave_control_responsive() {
    let media_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    media_listener.set_nonblocking(true).unwrap();
    let media_address = media_listener.local_addr().unwrap();
    let forwarding = station(media_address, CameraForwarding::Enabled);
    let direct_only = station(media_address, CameraForwarding::DirectOnly);
    let base = format!("http://{}", forwarding.local_address());
    let config = ControllerConfig::parse(include_str!(
        "../../server-core/tests/support/two_joint_config.json"
    ))
    .unwrap();
    let (link, link_endpoint) = Link::new(&config, Arc::new(Lease::new(LEASE))).unwrap();
    let mut link_endpoint = Some(link_endpoint);
    forwarding.attach_arms(BTreeMap::from([("follower".into(), link.handle())]));
    let shutdown = Arc::new(AtomicBool::new(false));
    let controller_shutdown = shutdown.clone();
    let controller = std::thread::spawn(move || {
        hebbian_rua_core::control_loop::run(
            &mut NoPhysicsStandIn::at(vec![0.0, 0.0], 0.02),
            &mut link_endpoint,
            &config,
            Some(Duration::from_secs(5)),
            &controller_shutdown,
        );
    });
    let upstream_requests = Arc::new(Semaphore::new(0));
    let media = axum::Router::new()
        .route(
            "/media/status",
            get(|| async {
                axum::Json(json!({
                    "service": {"state": "running"},
                    "cameras": {
                        "wrist": {"state": "ambiguous"},
                        "front": {"state": "capturing", "publication": {"state": "reconnecting", "detail": null}}
                    }
                }))
            }),
        )
        .route(
            "/cameras",
            get(|| async { axum::Json(json!([{"name": "wrist", "available": true}])) }),
        )
        .route(
            "/cameras/wrist",
            get(|| async {
                (
                    [
                        ("content-type", "image/jpeg"),
                        ("x-captured-at-unix-ns", "12345"),
                    ],
                    "jpeg",
                )
            }),
        )
        .route(
            "/cameras/stalled",
            get({
                let upstream_requests = upstream_requests.clone();
                move || async move {
                    upstream_requests.add_permits(1);
                    std::future::pending::<StatusCode>().await
                }
            }),
        );
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let listener = tokio::net::TcpListener::from_std(media_listener).unwrap();
            let media_task = tokio::spawn(async move { axum::serve(listener, media).await });
            assert_forwarding(
                &base,
                upstream_requests,
                media_address,
                direct_only.local_address(),
            )
            .await;
            media_task.abort();
            let _ = media_task.await;
            let response = Client::new()
                .get(format!("{base}/media/status"))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
            assert_eq!(
                response.json::<serde_json::Value>().await.unwrap(),
                json!({"detail": "Camera status could not be refreshed"})
            );
        });
    shutdown.store(true, Ordering::Release);
    controller.join().unwrap();
}

#[allow(
    clippy::too_many_lines,
    reason = "The HTTP assertions share the same live station and failure scenario."
)]
async fn assert_forwarding(
    base: &str,
    upstream_requests: Arc<Semaphore>,
    media_address: SocketAddr,
    direct_only_address: SocketAddr,
) {
    let client = Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(3))
        .build()
        .unwrap();
    let status: serde_json::Value = client
        .get(format!("{base}/media/status"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        status,
        json!({
            "service": {"state": "running"},
            "cameras": {
                "wrist": {"state": "ambiguous"},
                "front": {"state": "capturing", "publication": {"state": "reconnecting", "detail": null}}
            }
        })
    );
    let cameras: Vec<CameraListing> = client
        .get(format!("{base}/cameras"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(cameras.len(), 1);
    assert_eq!(cameras[0].name, "wrist");
    assert!(cameras[0].available);
    let frame = client
        .get(format!("{base}/cameras/wrist"))
        .send()
        .await
        .unwrap();
    assert_eq!(frame.status(), StatusCode::OK);
    assert_eq!(frame.headers()["content-type"], "image/jpeg");
    assert_eq!(frame.headers()["x-captured-at-unix-ns"], "12345");
    assert_eq!(frame.bytes().await.unwrap(), "jpeg");
    let instance: StationInstance = client
        .get(format!("{base}/instance"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let stalled: Vec<_> = (0..4)
        .map(|_| tokio::spawn(client.get(format!("{base}/cameras/stalled")).send()))
        .collect();
    tokio::time::timeout(Duration::from_secs(2), upstream_requests.acquire_many(4))
        .await
        .unwrap()
        .unwrap()
        .forget();
    tokio::time::timeout(Duration::from_millis(400), async {
        assert_eq!(
            client
                .get(format!("{base}/cameras"))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(
            client
                .get(format!("{base}/state"))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        let stop: CommandAnswer = client
            .post(format!("{base}/commands"))
            .json(&json!({
                "operation_id": uuid::Uuid::new_v4().to_string(),
                "station_instance_id": instance.id,
                "arm": "follower",
                "command": {"kind": "stop"}
            }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert!(matches!(stop, CommandAnswer::Accepted { .. }), "{stop:?}");
    })
    .await
    .unwrap();
    for request in stalled {
        assert_eq!(
            request.await.unwrap().unwrap().status(),
            StatusCode::GATEWAY_TIMEOUT
        );
    }
    assert_eq!(
        client
            .get(format!("{base}/cameras"))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );

    assert_eq!(
        client
            .get(format!("http://{direct_only_address}/cameras"))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        client
            .get(format!("http://{media_address}/cameras"))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
}

#[test]
fn camera_devices_skips_non_executable_media_companion_on_path()
-> Result<(), Box<dyn std::error::Error>> {
    use std::{fs, os::unix::fs::PermissionsExt, process::Command};

    let directory = tempfile::tempdir()?;
    let station = directory.path().join("station");
    fs::copy(env!("CARGO_BIN_EXE_hebbian-rua-server"), &station)?;

    let blocked = directory.path().join("blocked");
    let working = directory.path().join("working");
    fs::create_dir(&blocked)?;
    fs::create_dir(&working)?;
    let program = "hebbian-rua-media";
    fs::write(blocked.join(program), b"not executable")?;
    let companion = working.join(program);
    fs::write(&companion, b"#!/bin/sh\nprintf '%s\\n' \"$1\"\n")?;
    fs::set_permissions(&companion, fs::Permissions::from_mode(0o755))?;

    let output = Command::new(station)
        .arg("camera-devices")
        .current_dir(directory.path())
        .env_remove("HEBBIAN_MEDIA")
        .env("PATH", std::env::join_paths([blocked, working])?)
        .output()?;
    assert!(output.status.success(), "{output:?}");
    assert_eq!(output.stdout, b"camera-devices\n");
    Ok(())
}

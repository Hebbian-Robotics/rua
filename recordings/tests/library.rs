#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    reason = "This integration-test target may abort on failed setup or unexpected outcomes."
)]

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use recordings::{RecordingLibrary, router};
use serde_json::Value;
use std::fs;
use tempfile::TempDir;
use tower::ServiceExt;
const FILE: &str = "take/recording.mcap";
const DATA: &[u8] = b"not a real mcap, but long enough to range over";
fn fixture() -> (TempDir, RecordingLibrary) {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("take")).unwrap();
    fs::write(root.path().join(FILE), DATA).unwrap();
    fs::create_dir_all(root.path().join(".hidden")).unwrap();
    for directory in ["src", "tests"] {
        fs::create_dir(root.path().join(directory)).unwrap();
        fs::write(
            root.path().join(directory).join("keep.txt"),
            b"source files",
        )
        .unwrap();
    }
    let library = RecordingLibrary::new(root.path());
    (root, library)
}
#[test]
fn lists_each_recording_with_its_data_file() {
    let (_root, library) = fixture();
    let listed = library.recordings().unwrap();
    assert_eq!(listed.recordings.len(), 1);
    let recording = &listed.recordings[0];
    assert_eq!(recording.name, "take");
    assert_eq!(recording.mcap_path.as_deref(), Some(FILE));
    assert_eq!(recording.size_bytes, DATA.len() as u64);
}
#[cfg(target_os = "linux")]
#[test]
fn skips_recording_directories_with_non_utf8_names() {
    use std::os::unix::ffi::OsStrExt as _;

    let (root, library) = fixture();
    let invalid_name = std::ffi::OsStr::from_bytes(b"bad-\xff");
    fs::create_dir(root.path().join(invalid_name)).unwrap();

    let listed = library.recordings().unwrap();
    assert_eq!(listed.recordings.len(), 1);
    assert_eq!(listed.recordings[0].name, "take");
}

#[tokio::test]
async fn serves_ranges_and_refuses_traversal_before_deleting_a_listed_recording() {
    let (root, library) = fixture();
    let app = router(library);
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/recordings")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let listing: Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(listing["recordings"][0]["mcapPath"], FILE);
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/recordings/{FILE}"))
                .header("Range", "bytes=4-11")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(
        response.headers()["content-range"],
        format!("bytes 4-11/{}", DATA.len())
    );
    assert_eq!(
        &response.into_body().collect().await.unwrap().to_bytes()[..],
        &DATA[4..12]
    );
    for path in [
        "..",
        "%2e%2e",
        "take%2f..",
        "%2fetc",
        ".hidden",
        "take%2f..%2foutside",
        "unknown",
        "src",
        "tests",
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/recordings/{path}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
    }
    #[cfg(unix)]
    {
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("secret.mcap"), b"secret").unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join("link")).unwrap();
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/recordings/link/secret.mcap")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
    let response = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/recordings/take")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert!(!root.path().join("take").exists());
    for directory in ["src", "tests"] {
        assert_eq!(
            fs::read(root.path().join(directory).join("keep.txt")).unwrap(),
            b"source files"
        );
    }
}
#[cfg(unix)]
#[test]
fn standalone_viewer_serves_the_bundle_and_api_then_stops_on_sigterm() {
    use std::io::{BufRead, Read, Write};
    struct RunningViewer(std::process::Child);
    impl Drop for RunningViewer {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let (root, _) = fixture();
    let viewer_directory = root.path().join("viewer");
    fs::create_dir(&viewer_directory).unwrap();
    fs::write(viewer_directory.join("recording.html"), "recordings viewer").unwrap();
    let mut viewer = RunningViewer(
        std::process::Command::new(env!("CARGO_BIN_EXE_hebbian-rua-recordings"))
            .arg("--directory")
            .arg(root.path())
            .args(["--address", "127.0.0.1:0", "--viewer-directory"])
            .arg(viewer_directory)
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let mut started = String::new();
    std::io::BufReader::new(viewer.0.stdout.take().unwrap())
        .read_line(&mut started)
        .unwrap();
    let url = started.trim().strip_prefix("Recordings: http://").unwrap();
    let address = url.split('/').next().unwrap();
    let get = |path: &str| {
        let mut stream = std::net::TcpStream::connect(address).unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        write!(
            stream,
            "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
        )
        .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        response
    };
    assert!(get("/").starts_with("HTTP/1.1 307"));
    assert!(get("/viewer/").starts_with("HTTP/1.1 307"));
    assert!(get("/viewer/recording.html").ends_with("recordings viewer"));
    let listing = get("/api/recordings");
    assert!(listing.starts_with("HTTP/1.1 200"));
    assert!(listing.contains("\"name\":\"take\""));
    assert!(
        std::process::Command::new("kill")
            .args(["-TERM", &viewer.0.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    assert!(viewer.0.wait().unwrap().success());
}

#[tokio::test]
async fn active_recording_guards_follow_the_station_lifecycle_callback() {
    let (root, library) = fixture();
    fs::remove_file(root.path().join(FILE)).unwrap();
    let active = std::sync::Arc::new(std::sync::Mutex::new(Some("take".to_owned())));
    let current = active.clone();
    let app = router(library.with_active_recording(move || current.lock().unwrap().clone()));
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/recordings")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let listing: Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(listing["active"], "take");
    assert_eq!(listing["recordings"].as_array().unwrap().len(), 1);
    assert_eq!(listing["recordings"][0]["name"], "take");
    assert!(listing["recordings"][0]["mcapPath"].is_null());
    fs::write(root.path().join(FILE), DATA).unwrap();
    for (method, path, expected) in [
        (
            "DELETE",
            "/recordings/take".to_owned(),
            StatusCode::CONFLICT,
        ),
        ("GET", format!("/recordings/{FILE}"), StatusCode::OK),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
    }
    assert!(root.path().join("take").is_dir());
    *active.lock().unwrap() = None;
    let response = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/recordings/take")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
}

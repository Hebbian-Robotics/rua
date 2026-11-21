#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    reason = "This integration-test target may abort on failed setup or unexpected outcomes."
)]

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;

use std::sync::Arc;

use hebbian_robot_use_core::station_api::DiscardTelemetry;
use hebbian_robot_use_core::station_api::HttpServer;

fn scratch_directory(label: &str) -> PathBuf {
    let directory =
        std::env::temp_dir().join(format!("hebbian-http-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).unwrap();
    directory
}

fn get(address: SocketAddr, path: &str) -> (u16, Vec<u8>) {
    let mut stream = TcpStream::connect(address).unwrap();
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: test\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut response = Vec::new();
    stream.read_to_end(&mut response).unwrap();
    let header_end = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .unwrap();
    let head = String::from_utf8_lossy(&response[..header_end]).to_string();
    let status = head.split_whitespace().nth(1).unwrap().parse().unwrap();
    let mut body = response[header_end + 4..].to_vec();
    if head
        .to_ascii_lowercase()
        .contains("transfer-encoding: chunked")
    {
        body = dechunk(&body);
    }
    (status, body)
}

fn dechunk(mut chunked: &[u8]) -> Vec<u8> {
    let mut body = Vec::new();
    loop {
        let line_end = chunked.windows(2).position(|pair| pair == b"\r\n").unwrap();
        let size =
            usize::from_str_radix(std::str::from_utf8(&chunked[..line_end]).unwrap(), 16).unwrap();
        if size == 0 {
            return body;
        }
        let start = line_end + 2;
        body.extend_from_slice(&chunked[start..start + size]);
        chunked = &chunked[start + size + 2..];
    }
}

#[test]
fn station_without_arms_reports_unavailable() {
    let server = HttpServer::start(
        "127.0.0.1:0",
        Vec::new(),
        None,
        Arc::new(DiscardTelemetry),
        Arc::default(),
        |_, _| Ok(axum::Router::new()),
    )
    .unwrap();
    server.attach_arms(std::collections::BTreeMap::new());
    assert_eq!(get(server.local_address(), "/state").0, 503);
    assert_eq!(get(server.local_address(), "/events").0, 503);
    server.close();
}

#[test]
fn health_answers_and_a_trace_is_listed_and_served_compressed() {
    let directory = scratch_directory("serve");
    let traces = directory.join("traces");
    std::fs::create_dir_all(&traces).unwrap();
    let contents = "{\"cycle\":1}\n".repeat(1000);
    std::fs::write(traces.join("controller-1-trace.jsonl"), &contents).unwrap();
    std::fs::write(directory.join("secret.txt"), "not a trace").unwrap();
    let server = HttpServer::start(
        "127.0.0.1:0",
        vec![traces],
        None,
        Arc::new(DiscardTelemetry),
        Arc::default(),
        |_, _| Ok(axum::Router::new()),
    )
    .unwrap();
    let address = server.local_address();

    let (status, body) = get(address, "/health");
    assert_eq!(status, 200);
    assert_eq!(body, br#"{"status":"ok"}"#);

    let (status, body) = get(address, "/traces");
    assert_eq!(status, 200);
    let listed: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(listed[0]["name"], "controller-1-trace.jsonl");
    assert_eq!(listed[0]["sizeBytes"], contents.len());
    assert_eq!(listed.as_array().unwrap().len(), 1);

    let (status, body) = get(address, "/traces/controller-1-trace.jsonl");
    assert_eq!(status, 200);
    assert!(body.len() < contents.len());
    let mut decompressed = String::new();
    flate2::read::GzDecoder::new(body.as_slice())
        .read_to_string(&mut decompressed)
        .unwrap();
    assert_eq!(decompressed, contents);

    for outside in [
        "/traces/..%2Fsecret.txt",
        "/traces/%2E%2E%2Fsecret.txt",
        "/traces/secret.txt",
        "/traces/..",
    ] {
        assert_eq!(get(address, outside).0, 404, "{outside}");
    }
    server.close();
}

#[test]
fn an_address_in_use_is_refused_with_the_address_and_the_reason() {
    let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = taken.local_addr().unwrap().to_string();
    let Err(error) = HttpServer::start(
        &address,
        Vec::new(),
        None,
        Arc::new(DiscardTelemetry),
        Arc::default(),
        |_, _| Ok(axum::Router::new()),
    ) else {
        panic!("a second server bound {address}");
    };
    let message = format!("{error:#}");
    assert!(message.contains(&address), "{message}");
    assert!(message.contains("in use"), "{message}");
}

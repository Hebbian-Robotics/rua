#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    reason = "This integration-test target may abort on failed setup or unexpected outcomes."
)]

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use hebbian_rua_core::station_api::HttpServer;
use hebbian_rua_server::openapi::routes;
use hebbian_rua_server::web::{self, CameraForwarding, WebSettings};

fn probe(address: SocketAddr, path: &str) -> (u16, Option<String>) {
    let mut stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    write!(
        stream,
        "PATCH {path} HTTP/1.1\r\nHost: station.local\r\nConnection: close\r\nContent-Length: 0\r\n\r\n"
    )
    .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    let (head, _) = response.split_once("\r\n\r\n").unwrap();
    let status = head.split_whitespace().nth(1).unwrap().parse().unwrap();
    let allow = head.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("allow")
            .then(|| value.trim().to_owned())
    });
    (status, allow)
}

fn registered_paths(source: &str, prefix: &str) -> BTreeSet<String> {
    source
        .split(".route(")
        .skip(1)
        .filter_map(|call| {
            let literal = call.trim_start().strip_prefix('"')?;
            let path = &literal[..literal.find('"')?];
            Some(format!("{prefix}{path}").replace("{*", "{"))
        })
        .collect()
}

#[test]
fn the_openapi_document_matches_the_routers() {
    let scratch = std::env::temp_dir().join(format!("hebbian-openapi-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).unwrap();
    let settings = WebSettings {
        local_cameras_only: false,
        recordings_directory: scratch.join("recordings"),
        enable_recording: false,
        media_address: None,
        camera_forwarding: CameraForwarding::Enabled,
        media_executable: scratch.join("hebbian-rua-media"),
        room: None,
        phone_page_url: web::DEFAULT_PHONE_PAGE_URL.into(),
        manual_control: false,
        media_status: hebbian_rua_server::media_status::MediaStatus::default(),
    };
    let live_run = Arc::new(hebbian_rua_server::run_telemetry::LiveRunStore::default());
    let server = HttpServer::start(
        "127.0.0.1:0",
        Vec::new(),
        live_run.clone(),
        Arc::default(),
        |runtime, bound| {
            let (routes, _manager) = web::mount(runtime, bound, settings, || true);
            Ok(routes
                .merge(hebbian_rua_server::run_telemetry::routes(live_run))
                .merge(hebbian_rua_server::controller_supervisor::router(
                    hebbian_rua_server::controller_supervisor::ControllerSupervisor::unmanaged(),
                )))
        },
    )
    .unwrap();

    let mut documented: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut media_paths = BTreeSet::new();
    for route in routes() {
        if route.direct_media {
            media_paths.insert(route.path.to_owned());
        }
        documented
            .entry(route.path.to_owned())
            .or_default()
            .insert(route.method.to_uppercase());
    }
    for (path, methods) in &documented {
        let filled = path.replace(['{', '}'], "");
        let (status, allow) = probe(server.local_address(), &filled);
        assert_eq!(status, 405, "{path} is documented but not routed");
        let mut routed_methods: BTreeSet<String> = allow
            .unwrap_or_default()
            .split(',')
            .map(|method| method.trim().to_owned())
            .filter(|method| !method.is_empty())
            .collect();
        // CONSTRAINT: Axum answers HEAD wherever it answers GET.
        if routed_methods.contains("GET") {
            routed_methods.remove("HEAD");
        }
        assert_eq!(&routed_methods, methods, "methods of {path}");
    }

    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let read = |file: &str| std::fs::read_to_string(root.join(file)).unwrap();
    let mut registered = BTreeSet::new();
    registered.extend(registered_paths(
        &read("server-core/src/station_api/http.rs"),
        "",
    ));
    registered.extend(registered_paths(&read("station/src/web.rs"), ""));
    registered.extend(registered_paths(&read("station/src/camera_proxy.rs"), ""));
    registered.extend(registered_paths(&read("station/src/run_telemetry.rs"), ""));
    registered.extend(registered_paths(
        &read("station/src/controller_supervisor.rs"),
        "",
    ));
    registered.extend(registered_paths(&read("station/src/recording.rs"), ""));
    registered.extend(registered_paths(&read("recordings/src/routes.rs"), ""));
    assert_eq!(
        registered_paths(&read("livekit-cameras/src/snapshots.rs"), ""),
        media_paths,
        "direct media routes against documented overrides"
    );
    let all_documented: BTreeSet<String> = documented.keys().cloned().collect();
    assert_eq!(
        registered, all_documented,
        "routed paths against documented paths"
    );
}

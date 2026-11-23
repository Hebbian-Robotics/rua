//! In-process SO-101 station fixture shared by CLI and SDK outcome tests.
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use hebbian_robot_use_core::control_link::Link;
use hebbian_robot_use_core::control_loop::{ArmSetup, run_arms};
use hebbian_robot_use_core::lease::{LEASE, Lease};
use hebbian_robot_use_core::plant::Plant;
use hebbian_robot_use_core::profile::{JointLimitSource, controller_config, load_robot_profile};
use hebbian_robot_use_core::station_api::HttpServer;
use hebbian_robot_use_server::media_status::MediaStatus;
use hebbian_robot_use_server::run_telemetry::LiveRunStore;
use hebbian_robot_use_server::web::{self, CameraForwarding, WebSettings};
use hebbian_rua::{Error, Station};

type TestResult = Result<(), Box<dyn std::error::Error>>;

pub const ARM: &str = "arm";
pub const REST: [f64; 5] = [0.0, -0.5, 0.5, 0.0, 0.0];

/// A `LeRobot` calibration with every motor at the middle of its range.
fn write_calibration(root: &Path) -> TestResult {
    let directory = root.join("robots").join("so_follower");
    std::fs::create_dir_all(&directory)?;
    let entries: serde_json::Map<String, serde_json::Value> = (1..=6)
        .map(|id| {
            (
                format!("motor_{id}"),
                serde_json::json!({"id": id, "drive_mode": 0, "homing_offset": 0, "range_min": 100, "range_max": 4000}),
            )
        })
        .collect();
    std::fs::write(
        directory.join("arm.json"),
        serde_json::Value::Object(entries).to_string(),
    )?;
    Ok(())
}

pub struct SimulatedStation {
    server: Option<HttpServer>,
    shutdown: Arc<AtomicBool>,
    loop_thread: Option<JoinHandle<()>>,
    pub url: String,
}

impl SimulatedStation {
    pub async fn start<P: Plant + Send + 'static>(
        profiles: &Path,
        arm_names: &[&str],
        create_plant: impl Fn() -> P + Send + 'static,
        routes: axum::Router,
        media_address: Option<std::net::SocketAddr>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let directory = std::env::temp_dir().join(format!(
            "hebbian-robot-use-simulated-station-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&directory);
        write_calibration(&directory)?;
        let profile = load_robot_profile(profiles, "so101")?;
        let mut config =
            controller_config(&profile, ARM, JointLimitSource::Urdf, &directory)?.into_input();
        config.feetech = None;
        config.guard = None;
        config.enable_checks = None;
        config.trace_directory = directory.join("traces").to_string_lossy().into_owned();
        let shared_lease = Arc::new(Lease::new(LEASE));
        let links = arm_names
            .iter()
            .enumerate()
            .map(|(position, name)| {
                let mut config = config.clone();
                if arm_names.len() > 1 {
                    config.base_pose = Some(hebbian_rua::contracts::BasePose {
                        position_meters: [0.0, f64::from(u32::try_from(position)?), 0.0],
                        yaw_radians: 0.0,
                    });
                }
                let config = hebbian_robot_use_core::config::ControllerConfig::try_from(config)?;
                let link = Link::new(&config, Arc::clone(&shared_lease))?;
                Ok(((*name).to_owned(), config, link))
            })
            .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?;
        let (server, mounted) = tokio::task::spawn_blocking(move || {
            let mut mounted = None;
            HttpServer::start(
                "127.0.0.1:0",
                vec![],
                None,
                Arc::new(LiveRunStore::default()),
                Arc::default(),
                |runtime, address| {
                    let (web_routes, web) = web::mount(
                        runtime,
                        address,
                        WebSettings {
                            viewer_directory: None,
                            local_cameras_only: false,
                            recordings_directory: std::env::temp_dir(),
                            enable_recording: false,
                            media_address,
                            camera_forwarding: CameraForwarding::DirectOnly,
                            media_executable: "unused".into(),
                            room: None,
                            phone_page_url: web::DEFAULT_PHONE_PAGE_URL.into(),
                            manual_control: false,
                            media_status: MediaStatus::default(),
                        },
                        || true,
                    );
                    mounted = Some(web);
                    Ok(routes.fallback_service(web_routes))
                },
            )
            .map(|server| (server, mounted))
        })
        .await??;
        let handles = links
            .iter()
            .map(|(name, _, link)| (name.clone(), link.handle()))
            .collect();
        if let Some(mounted) = mounted {
            mounted.attach_arms(&server.runtime_handle(), &handles);
        }
        server.attach_arms(handles);
        let shutdown = Arc::new(AtomicBool::new(false));
        let loop_shutdown = Arc::clone(&shutdown);
        let loop_thread = std::thread::spawn(move || {
            let mut setups = links
                .into_iter()
                .map(|(name, config, link)| ArmSetup::new(name, config, create_plant(), link))
                .collect::<Vec<_>>();
            run_arms(&mut setups, Some(Duration::from_secs(120)), &loop_shutdown);
            for setup in setups {
                setup.link.close();
            }
        });
        let url = format!("http://{}", server.local_address());
        Ok(Self {
            server: Some(server),
            shutdown,
            loop_thread: Some(loop_thread),
            url,
        })
    }

    /// A connection to a station whose arms have reported their first sample; a
    /// command sent earlier is refused with "No feedback from the arm yet".
    pub async fn connect(&self) -> Result<Station, Error> {
        let station = Station::connect(&self.url).await?;
        station
            .wait_for_state(|state| {
                state
                    .arms
                    .iter()
                    .all(|arm| arm.sampled_at_unix_seconds.is_some())
            })
            .await?;
        Ok(station)
    }
}

impl Drop for SimulatedStation {
    /// The server owns a runtime, which cannot be dropped inside an async test.
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        let server = self.server.take();
        let loop_thread = self.loop_thread.take();
        let _ = std::thread::spawn(move || {
            drop(server);
            if let Some(loop_thread) = loop_thread {
                let _ = loop_thread.join();
            }
        })
        .join();
    }
}

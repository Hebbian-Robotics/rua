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
use hebbian_robot_use_core::profile::{JointLimitSource, executor_config, load_robot_profile};
use hebbian_robot_use_core::station_api::HttpServer;
use hebbian_robot_use_sdk::{Error, Station};
use hebbian_robot_use_server::run_telemetry::LiveRunStore;

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
            executor_config(&profile, ARM, JointLimitSource::Urdf, &directory)?.into_input();
        config.feetech = None;
        config.guard = None;
        config.enable_checks = None;
        config.trace_directory = directory.join("traces").to_string_lossy().into_owned();
        let config = hebbian_robot_use_core::config::ExecutorConfig::try_from(config)?;
        let shared_lease = Arc::new(Lease::new(LEASE));
        let links = arm_names
            .iter()
            .map(|name| {
                Link::new(&config, Arc::clone(&shared_lease)).map(|link| ((*name).to_owned(), link))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let server = tokio::task::spawn_blocking(|| {
            HttpServer::start(
                "127.0.0.1:0",
                vec![],
                None,
                Arc::new(LiveRunStore::default()),
                Arc::default(),
                |_, _| Ok(routes),
            )
        })
        .await??;
        server.attach_arms(
            links
                .iter()
                .map(|(name, link)| (name.clone(), link.handle()))
                .collect(),
        );
        let shutdown = Arc::new(AtomicBool::new(false));
        let loop_shutdown = Arc::clone(&shutdown);
        let loop_thread = std::thread::spawn(move || {
            let mut setups = links
                .into_iter()
                .map(|(name, link)| ArmSetup::new(name, config.clone(), create_plant(), Some(link)))
                .collect::<Vec<_>>();
            run_arms(&mut setups, Some(Duration::from_secs(120)), &loop_shutdown);
            for setup in setups {
                if let Some(link) = setup.link {
                    link.close();
                }
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

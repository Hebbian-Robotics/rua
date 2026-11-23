#![allow(
    clippy::disallowed_types,
    reason = "Controller setup constructs hardware plants and transfers ownership to the control loop."
)]

use std::collections::BTreeMap;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, ensure};
use hdrhistogram::Histogram;
use hebbian_robot_use_core::config::ControllerConfig;
use hebbian_robot_use_core::control_link::{Link, LinkHandle};
use hebbian_robot_use_core::control_loop::{ArmSetup, Report, run_arms};
use hebbian_robot_use_core::guard::GuardedPlant;
use hebbian_robot_use_core::lease::{LEASE, Lease};
use hebbian_robot_use_core::plant::{NoPhysicsStandIn, Plant};
use hebbian_robot_use_core::sim_plant::SocketPlant;
use hebbian_robot_use_core::so101::driver::BusTiming;
use hebbian_robot_use_core::station_api::{self, HttpServer};
use hebbian_robot_use_server::controller_process::{ControllerLaunch, PlantSelection};
use hebbian_robot_use_server::controller_supervisor;
use hebbian_robot_use_server::run_telemetry::{self, LiveRunStore};
use hebbian_robot_use_server::web::{self, WebSettings};
use signal_hook::consts::{SIGINT, SIGTERM};

use crate::tools::{feetech_servos, load_config};

fn millisecond_percentiles(
    microseconds: &Histogram<u64>,
) -> serde_json::Map<String, serde_json::Value> {
    let milliseconds = |value: u64| std::time::Duration::from_micros(value).as_secs_f64() * 1000.0;
    if microseconds.is_empty() {
        return serde_json::Map::new();
    }
    [
        ("median_ms", microseconds.value_at_quantile(0.5)),
        ("p99_ms", microseconds.value_at_quantile(0.99)),
        ("p999_ms", microseconds.value_at_quantile(0.999)),
        ("max_ms", microseconds.max()),
    ]
    .into_iter()
    .map(|(name, value)| (name.to_owned(), milliseconds(value).into()))
    .collect()
}

pub(crate) fn bus_timing_json(timing: &BusTiming) -> serde_json::Value {
    let mut summary = millisecond_percentiles(&timing.answered_round_trip_microseconds);
    summary.insert(
        "answered".to_owned(),
        timing.answered_round_trip_microseconds.len().into(),
    );
    summary.insert("retries".to_owned(), timing.retries.into());
    summary.insert("failed_requests".to_owned(), timing.failed_requests.into());
    serde_json::Value::Object(summary)
}

fn print_timing(arm: &str, report: &Report, bus: Option<&BusTiming>) {
    let mut timing = millisecond_percentiles(&report.cycle_microseconds);
    timing.insert("arm".to_owned(), arm.into());
    timing.insert("cycles".to_owned(), report.cycle_microseconds.len().into());
    timing.insert("overruns".to_owned(), report.overruns.into());
    timing.insert("tripped".to_owned(), report.trip.clone().into());
    if let Some(bus) = bus {
        timing.insert("bus".to_owned(), bus_timing_json(bus));
    }
    if let Some(trace) = &report.trace {
        timing.insert(
            "trace".to_owned(),
            serde_json::json!({
                "path": trace.path,
                "written": trace.written,
                "dropped": trace.dropped,
                "error": trace.error,
            }),
        );
    }
    println!("{}", serde_json::Value::Object(timing));
}

// CONSTRAINT: The first termination signal requests clean shutdown; a second exits immediately
// if the loop cannot stop.
fn shutdown_flag() -> anyhow::Result<Arc<AtomicBool>> {
    let shutdown_requested = Arc::new(AtomicBool::new(false));
    for signal in [SIGINT, SIGTERM] {
        signal_hook::flag::register_conditional_shutdown(
            signal,
            1,
            Arc::clone(&shutdown_requested),
        )?;
        signal_hook::flag::register(signal, Arc::clone(&shutdown_requested))?;
    }
    Ok(shutdown_requested)
}

pub(crate) const DEFAULT_STAND_IN_LAG_SECONDS: f64 = 0.05;

pub(crate) struct ConfiguredArm {
    pub name: String,
    pub config: ControllerConfig,
    pub config_path: PathBuf,
}

pub(crate) fn load_arms(arms: &[(String, PathBuf)]) -> anyhow::Result<Vec<ConfiguredArm>> {
    arms.iter()
        .map(|(name, path)| {
            Ok(ConfiguredArm {
                name: name.clone(),
                config: load_config(path)?,
                config_path: path.clone(),
            })
        })
        .collect()
}

fn arm_setup<P: Plant>(
    name: &str,
    config: ControllerConfig,
    plant: P,
    control: &Arc<Lease>,
) -> ArmSetup<P> {
    let link = Link::for_controller(&config, Arc::clone(control));
    ArmSetup::new(name.to_owned(), config, plant, link)
}

fn simulator_setups(
    arms: &[ConfiguredArm],
    directory: &Path,
    should_stop: impl Fn() -> bool,
    control: &Arc<Lease>,
) -> anyhow::Result<Vec<ArmSetup<SocketPlant>>> {
    // INVARIANT: Every arm's plant socket exists before the first accept, so a
    // simulator serving several arms can connect to all of them at once.
    let mut listeners = Vec::new();
    for arm in arms {
        let name = &arm.name;
        let config = arm.config.clone();
        let plant_path = directory.join(format!("{name}.plant.sock"));
        let _ = std::fs::remove_file(&plant_path);
        let listener = UnixListener::bind(&plant_path)
            .with_context(|| format!("Cannot listen on {}", plant_path.display()))?;
        listeners.push((name, config, listener));
    }
    let mut setups = Vec::new();
    for (name, config, listener) in listeners {
        let timeouts = config
            .simulator_timeouts()
            .context("An arm on a simulator needs a sim_plant section")?;
        tracing::info!(arm = name, "Waiting for the simulator to connect");
        let plant = SocketPlant::accept(
            &listener,
            &config.driven_joints,
            config.joint_count,
            timeouts,
            &should_stop,
        )
        .with_context(|| format!("Arm {name}'s simulator"))?;
        setups.push(arm_setup(name, config, plant, control));
    }
    Ok(setups)
}

fn hardware_setups(
    arms: &[ConfiguredArm],
    control: &Arc<Lease>,
) -> anyhow::Result<
    Vec<
        ArmSetup<
            GuardedPlant<
                hebbian_robot_use_core::so101::driver::Servos<
                    hebbian_robot_use_core::so101::driver::SerialBus,
                >,
            >,
        >,
    >,
> {
    let mut setups = Vec::new();
    for arm in arms {
        let name = &arm.name;
        let config = arm.config.clone();
        let guard_config_path = config.guard.as_ref().map(|_| arm.config_path.as_path());
        let servos = feetech_servos(&config)?;
        let plant = GuardedPlant::new(servos, guard_config_path);
        setups.push(arm_setup(name, config, plant, control));
    }
    Ok(setups)
}

fn no_physics_setups(
    arms: &[ConfiguredArm],
    plant_lag_seconds: f64,
    control: &Arc<Lease>,
) -> Vec<ArmSetup<NoPhysicsStandIn>> {
    let mut setups = Vec::new();
    for arm in arms {
        let name = &arm.name;
        let config = arm.config.clone();
        let start: Vec<f64> = config
            .lower_limits
            .iter()
            .zip(&config.upper_limits)
            .map(|(lower, upper)| 0.0_f64.clamp(*lower, *upper))
            .collect();
        let mut plant = NoPhysicsStandIn::at(start, plant_lag_seconds);
        if let Some(gripper) = &config.gripper {
            plant = plant
                .with_gripper(0.0_f64.clamp(gripper.minimum_radians(), gripper.maximum_radians()));
        }
        setups.push(arm_setup(name, config, plant, control));
    }
    setups
}

fn validate_arms(arms: &[ConfiguredArm]) -> anyhow::Result<()> {
    ensure!(!arms.is_empty(), "At least one arm is required");
    hebbian_robot_use_core::profile::validate_arm_names(arms.iter().map(|arm| arm.name.as_str()))
}

pub(crate) fn run_supervised(
    arms: &[ConfiguredArm],
    directory: &Path,
    plant: PlantSelection,
    http_address: &str,
    web: WebSettings,
    media: crate::media::MediaPlan,
) -> anyhow::Result<()> {
    validate_arms(arms)?;
    let shutdown_requested = shutdown_flag()?;
    let lease = Arc::new(Lease::new(LEASE));
    let mut links = BTreeMap::new();
    let mut handoffs = BTreeMap::new();
    let mut controller_arms = Vec::new();
    for arm in arms {
        let link = Arc::new(Link::new(&arm.config, Arc::clone(&lease))?);
        let handoff = link
            .take_controller_station_arm()
            .context("The station handoff was already taken")?;
        handoffs.insert(arm.name.clone(), handoff);
        links.insert(arm.name.clone(), link);
        controller_arms.push((arm.name.clone(), arm.config_path.clone()));
    }
    let session = hebbian_robot_use_core::controller_bridge::StationSession::new(handoffs);
    let launch = ControllerLaunch {
        executable: hebbian_robot_use_core::running_executable_for_child()?,
        arms: controller_arms,
        work_directory: directory.to_path_buf(),
        plant,
    };
    let (supervisor, supervisor_thread) = controller_supervisor::start(
        launch,
        session,
        links.clone(),
        Arc::clone(&lease),
        Arc::clone(&shutdown_requested),
    );
    let directories = station_api::trace_directories(
        arms.iter()
            .map(|arm| Path::new(arm.config.trace_directory.as_str())),
    );
    let mut mounted = None;
    let media_status = web.media_status.clone();
    let local_cameras_only = web.local_cameras_only;
    let mut web = web;
    let live_run = Arc::new(LiveRunStore::default());
    let server = HttpServer::start(
        http_address,
        directories,
        web.viewer_directory.take(),
        live_run.clone(),
        Arc::clone(&shutdown_requested),
        |runtime, bound| {
            let (routes, mounted_routes) =
                web::mount(runtime, bound, web, supervisor.availability_reader());
            supervisor.attach_recording(mounted_routes.recording.clone());
            mounted = Some(mounted_routes);
            Ok(routes
                .merge(run_telemetry::routes(live_run))
                .merge(controller_supervisor::router(supervisor)))
        },
    );
    let server = match server {
        Ok(server) => server,
        Err(error) => {
            shutdown_requested.store(true, Ordering::Release);
            let _ = supervisor_thread.join();
            return Err(error);
        }
    };
    let mounted = mounted.context("The station routes were not mounted")?;
    let handles = links
        .iter()
        .map(|(name, link)| (name.clone(), link.handle()))
        .collect();
    mounted.attach_arms(&server.runtime_handle(), &handles);
    server.attach_arms(handles);
    let runtime = server.runtime_handle();
    let media = crate::media::MediaSupervisor::start(
        &runtime,
        &media,
        &web::local_http_url(server.local_address()),
        &media_status,
        local_cameras_only,
    );
    // CONSTRAINT: Launchers read the bound address from this startup announcement.
    eprintln!("HTTP on http://{}", server.local_address());
    while !shutdown_requested.load(Ordering::Acquire) {
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = supervisor_thread.join();
    runtime.block_on(async {
        mounted.recording.shutdown().await;
        media.shutdown().await;
    });
    server.close();
    for link in links.into_values() {
        if let Ok(link) = Arc::try_unwrap(link) {
            link.close();
        }
    }
    Ok(())
}

fn run_connected<P: Plant>(
    setups: &mut [ArmSetup<P>],
    socket_path: &Path,
    instance: uuid::Uuid,
    shutdown_requested: &Arc<AtomicBool>,
    verify_torque: bool,
) -> anyhow::Result<Vec<(String, Report)>> {
    for setup in setups.iter_mut() {
        setup.verify_disabled(verify_torque)?;
    }
    let socket =
        UnixStream::connect(socket_path).context("Cannot connect to the supervising station")?;
    let stop_socket = socket.try_clone()?;
    let arms: BTreeMap<String, LinkHandle> = setups
        .iter()
        .map(|setup| (setup.name.clone(), setup.link.handle()))
        .collect();
    let shutting_down = Arc::new(AtomicBool::new(false));
    let service_shutting_down = Arc::clone(&shutting_down);
    let service_shutdown_requested = Arc::clone(shutdown_requested);
    let service = std::thread::spawn(move || {
        let result = hebbian_robot_use_core::controller_service::serve_controller(
            socket,
            hebbian_robot_use_core::controller_transport::ControllerInstanceId(
                *instance.as_bytes(),
            ),
            &arms,
            &service_shutdown_requested,
        );
        if !service_shutting_down.load(Ordering::Acquire)
            && !service_shutdown_requested.load(Ordering::Acquire)
        {
            tracing::error!(?result, "Station connection lost");
            // INVARIANT: The guard, when present, takes over after the controller exits.
            // Do not run the normal return-to-rest shutdown without the station.
            std::process::exit(1);
        }
    });
    let reports = run_arms(setups, None, shutdown_requested);
    shutting_down.store(true, Ordering::Release);
    let _ = stop_socket.shutdown(std::net::Shutdown::Both);
    let _ = service.join();
    Ok(reports)
}

fn run_setups<P: Plant>(
    mut setups: Vec<ArmSetup<P>>,
    station_socket: &Path,
    instance: uuid::Uuid,
    shutdown_requested: &Arc<AtomicBool>,
    hardware: bool,
    bus_timing: impl Fn(&P) -> Option<&BusTiming>,
) -> anyhow::Result<()> {
    let reports = run_connected(
        &mut setups,
        station_socket,
        instance,
        shutdown_requested,
        hardware,
    )?;
    for (setup, (name, report)) in setups.into_iter().zip(&reports) {
        print_timing(name, report, bus_timing(setup.plant()));
        setup.link.close();
    }
    Ok(())
}

fn select_plant(
    arms: &[ConfiguredArm],
    socket_directory: Option<PathBuf>,
    plant_lag_seconds: f64,
    hardware: bool,
) -> anyhow::Result<PlantSelection> {
    ensure!(
        plant_lag_seconds.is_finite() && plant_lag_seconds > 0.0,
        "--plant-lag-seconds must be positive"
    );
    let uses_simulator = |arm: &ConfiguredArm| arm.config.simulator_timeouts().is_some();
    if arms.iter().any(uses_simulator) {
        ensure!(
            !hardware && arms.iter().all(uses_simulator),
            "Arms with a sim_plant section cannot share a controller with arms on other plants"
        );
        Ok(PlantSelection::Simulator {
            socket_directory: socket_directory
                .context("Arms on a simulator need --socket-directory for their plant sockets")?,
        })
    } else if hardware {
        if let Some(arm) = arms.iter().find(|arm| arm.config.feetech.is_none()) {
            anyhow::bail!("Arm {:?} needs a feetech section with --hardware", arm.name);
        }
        Ok(PlantSelection::Servos)
    } else {
        Ok(PlantSelection::NoPhysicsStandIn {
            lag_seconds: plant_lag_seconds,
        })
    }
}

pub(crate) fn run_configured(
    arms: &[ConfiguredArm],
    station_socket: &Path,
    station_pid: u32,
    instance: uuid::Uuid,
    socket_directory: Option<PathBuf>,
    plant_lag_seconds: f64,
    hardware: bool,
) -> anyhow::Result<()> {
    validate_arms(arms)?;
    let plant = select_plant(arms, socket_directory, plant_lag_seconds, hardware)?;
    let shutdown_requested = shutdown_flag()?;
    let control = Arc::new(Lease::new(LEASE));
    match plant {
        PlantSelection::Servos => run_setups(
            hardware_setups(arms, &control)?,
            station_socket,
            instance,
            &shutdown_requested,
            true,
            |plant| Some(plant.inner().bus().timing()),
        ),
        PlantSelection::Simulator { socket_directory } => run_setups(
            simulator_setups(
                arms,
                &socket_directory,
                || {
                    shutdown_requested.load(Ordering::Acquire)
                        || std::os::unix::process::parent_id() != station_pid
                },
                &control,
            )?,
            station_socket,
            instance,
            &shutdown_requested,
            false,
            |_| None,
        ),
        PlantSelection::NoPhysicsStandIn { lag_seconds } => run_setups(
            no_physics_setups(arms, lag_seconds, &control),
            station_socket,
            instance,
            &shutdown_requested,
            false,
            |_| None,
        ),
    }
}

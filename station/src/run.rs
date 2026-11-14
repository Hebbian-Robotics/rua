//! The `run` command: one loop over every arm, on a simulator's socket, the
//! Feetech servos, or the lag simulator, and the timing each arm reports.

use std::collections::BTreeMap;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, ensure};
use hdrhistogram::Histogram;
use hebbian_robot_use_server::config::ExecutorConfig;
use hebbian_robot_use_server::control_link::{Link, LinkHandle};
use hebbian_robot_use_server::control_loop::{ArmSetup, Report, run_arms};
use hebbian_robot_use_server::guard::GuardedPlant;
use hebbian_robot_use_server::lease::{LEASE, Lease};
use hebbian_robot_use_server::plant::{Plant, SimulatedPlant};
use hebbian_robot_use_server::sim_plant::{SimTimeouts, SocketPlant};
use hebbian_robot_use_server::so101::driver::BusTiming;
use hebbian_robot_use_server::station_api::{self, HttpServer};
use hebbian_station::controller_process::ControllerLaunch;
use hebbian_station::controller_supervisor;
use hebbian_station::run_telemetry::{self, LiveRunStore};
use hebbian_station::web::{self, Mounted, WebSettings};
use signal_hook::consts::{SIGINT, SIGTERM};

use crate::tools::{feetech_servos, load_config};

/// The HTTP server and its mounted viewer routes.
struct Serving {
    server: HttpServer,
    mounted: Mounted,
}

impl Serving {
    /// Stop any recording, then stop the server.
    fn close(self) {
        let handle = self.server.runtime_handle();
        handle.block_on(self.mounted.recording.shutdown());
        self.server.close();
    }
}

/// The median, tail and longest of durations recorded in microseconds, in
/// milliseconds; empty when nothing was recorded.
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

/// What the serial bus answered: the round trips of answered requests, and the
/// retries and failures, for choosing the serial timeout on a real arm.
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

/// Set on the first SIGINT or SIGTERM so the loop ends cleanly; a second one
/// ends the process at once, for a loop that does not stop.
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

/// A validated arm configuration and the file needed by its hardware guard.
pub(crate) struct ConfiguredArm {
    pub name: String,
    pub config: ExecutorConfig,
    pub guard_config_path: Option<PathBuf>,
}

pub(crate) fn load_arms(arms: &[(String, PathBuf)]) -> anyhow::Result<Vec<ConfiguredArm>> {
    arms.iter()
        .map(|(name, path)| {
            Ok(ConfiguredArm {
                name: name.clone(),
                config: load_config(path)?,
                guard_config_path: Some(path.clone()),
            })
        })
        .collect()
}

/// One configured arm, its plant, and its link when served.
fn arm_setup<P: Plant>(
    name: &str,
    config: ExecutorConfig,
    plant: P,
    with_link: bool,
    control: &Arc<Lease>,
) -> anyhow::Result<ArmSetup<P>> {
    let link = with_link
        .then(|| Link::new(&config, Arc::clone(control)))
        .transpose()?;
    Ok(ArmSetup {
        name: name.to_owned(),
        config,
        plant,
        link,
    })
}

fn close_links<P: Plant>(setups: &mut [ArmSetup<P>]) {
    for setup in setups {
        if let Some(link) = setup.link.take() {
            link.close();
        }
    }
}
/// Run every arm's loop to its end, then close the links and print each arm's
/// timing; `bus_timing` reads what the plant's serial bus answered, if it has one.
fn run_and_report<P: Plant>(
    mut setups: Vec<ArmSetup<P>>,
    run_for: Option<Duration>,
    shutdown_requested: &AtomicBool,
    serving: Option<&Serving>,
    bus_timing: impl Fn(&P) -> Option<&BusTiming>,
) {
    if let Some(serving) = serving {
        let arms: BTreeMap<String, LinkHandle> = setups
            .iter()
            .filter_map(|setup| {
                setup
                    .link
                    .as_ref()
                    .map(|link| (setup.name.clone(), link.handle()))
            })
            .collect();
        serving
            .mounted
            .attach_arms(&serving.server.runtime_handle(), &arms);
        serving.server.attach_arms(arms);
    }
    let reports = run_arms(&mut setups, run_for, shutdown_requested);
    close_links(&mut setups);
    for (setup, (name, report)) in setups.iter().zip(&reports) {
        print_timing(name, report, bus_timing(&setup.plant));
    }
}

/// Arms whose plants are external simulators, each connected on its plant socket.
fn simulator_setups(
    arms: &[ConfiguredArm],
    directory: &Path,
    with_link: bool,
    shutdown_requested: &AtomicBool,
    control: &Arc<Lease>,
) -> anyhow::Result<Vec<ArmSetup<SocketPlant>>> {
    // Every arm's plant socket exists before the first accept, so a
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
        let sim = config
            .sim_plant
            .clone()
            .context("An arm on a simulator needs a sim_plant section")?;
        eprintln!("Waiting for a simulator to connect for arm {name}...");
        let plant = SocketPlant::accept(
            &listener,
            &config.driven_joints,
            config.joint_count,
            SimTimeouts {
                connect: Duration::from_secs_f64(sim.connect_timeout_seconds),
                reply: Duration::from_secs_f64(sim.reply_timeout_seconds),
                real_time: sim.real_time,
            },
            shutdown_requested,
        )
        .with_context(|| format!("Arm {name}'s simulator"))?;
        setups.push(arm_setup(name, config, plant, with_link, control)?);
    }
    Ok(setups)
}

/// Arms on their Feetech servos. With a guard section, a guard process watches
/// each arm if this one stops.
fn hardware_setups(
    arms: &[ConfiguredArm],
    with_link: bool,
    control: &Arc<Lease>,
) -> anyhow::Result<
    Vec<
        ArmSetup<
            GuardedPlant<
                hebbian_robot_use_server::so101::driver::Servos<
                    hebbian_robot_use_server::so101::driver::SerialBus,
                >,
            >,
        >,
    >,
> {
    let mut setups = Vec::new();
    for arm in arms {
        let name = &arm.name;
        let config = arm.config.clone();
        let guard_config_path = if config.guard.is_some() {
            Some(
                arm.guard_config_path
                    .as_deref()
                    .context("A configured hardware guard needs its config file")?,
            )
        } else {
            None
        };
        let servos = feetech_servos(&config)?;
        let plant = GuardedPlant::new(servos, guard_config_path);
        setups.push(arm_setup(name, config, plant, with_link, control)?);
    }
    Ok(setups)
}

/// Arms on the lag simulator, starting at zero within their limits.
fn lag_setups(
    arms: &[ConfiguredArm],
    plant_lag_seconds: f64,
    with_link: bool,
    control: &Arc<Lease>,
) -> anyhow::Result<Vec<ArmSetup<SimulatedPlant>>> {
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
        let mut plant = SimulatedPlant::at(start, plant_lag_seconds);
        if let Some(gripper) = &config.gripper {
            plant = plant
                .with_gripper(0.0_f64.clamp(gripper.minimum_radians(), gripper.maximum_radians()));
        }
        setups.push(arm_setup(name, config, plant, with_link, control)?);
    }
    Ok(setups)
}

/// Keep the station alive while a separately supervised process owns the loop.
pub(crate) fn run_supervised(
    arms: &[ConfiguredArm],
    directory: &Path,
    plant_lag_seconds: f64,
    hardware: bool,
    http_address: &str,
    web: WebSettings,
) -> anyhow::Result<()> {
    ensure!(!arms.is_empty(), "A station needs at least one arm");
    let mut names: Vec<&str> = arms.iter().map(|arm| arm.name.as_str()).collect();
    names.sort_unstable();
    ensure!(
        names.windows(2).all(|pair| pair[0] != pair[1]),
        "Each --arm needs its own name"
    );
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
        controller_arms.push((
            arm.name.clone(),
            arm.guard_config_path
                .clone()
                .context("The controller needs its configuration file")?,
        ));
    }
    let session = hebbian_robot_use_server::controller_bridge::StationSession::new(handoffs);
    let launch = ControllerLaunch {
        executable: std::env::current_exe()?,
        arms: controller_arms,
        work_directory: directory.to_path_buf(),
        simulator_socket_directory: arms
            .iter()
            .any(|arm| arm.config.sim_plant.is_some())
            .then(|| directory.to_path_buf()),
        plant_lag_seconds,
        hardware,
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
    let live_run = Arc::new(LiveRunStore::default());
    let server = HttpServer::start(
        http_address,
        directories,
        station_api::default_viewer_directory(),
        live_run.clone(),
        |runtime, bound| {
            let (routes, mounted_routes) = web::mount(runtime, bound, web);
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
    eprintln!("HTTP on http://{}", server.local_address());
    while !shutdown_requested.load(Ordering::Acquire) {
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = supervisor_thread.join();
    server
        .runtime_handle()
        .block_on(mounted.recording.shutdown());
    server.close();
    for link in links.into_values() {
        if let Ok(link) = Arc::try_unwrap(link) {
            link.close();
        }
    }
    Ok(())
}

/// Verify fresh feedback and faults before announcing a controller as ready.
fn verify_controller_start<P: Plant>(
    setups: &mut [ArmSetup<P>],
    verify_torque: bool,
) -> anyhow::Result<()> {
    for setup in setups {
        setup
            .plant
            .preflight()
            .with_context(|| format!("{}: preflight failed", setup.name))?;
        let feedback = if verify_torque {
            let observation = setup
                .plant
                .observe_recovery()
                .with_context(|| format!("{}: cannot confirm torque", setup.name))?;
            let expected_motors =
                setup.config.joint_count + usize::from(setup.config.gripper.is_some());
            ensure!(
                observation.motor_torque.len() == expected_motors
                    && observation.motor_torque.iter().all(|(_, enabled)| !enabled),
                "{}: motor torque is not confirmed off",
                setup.name
            );
            if let Some(issue) = observation.health_issue {
                anyhow::bail!("{}: {issue}", setup.name);
            }
            observation.feedback
        } else {
            setup
                .plant
                .read_feedback()
                .with_context(|| format!("{}: cannot read feedback", setup.name))?
        };
        ensure!(
            feedback.positions.len() == setup.config.joint_count
                && feedback
                    .positions
                    .iter()
                    .all(|position| position.is_finite()),
            "{}: invalid joint feedback",
            setup.name
        );
        if let Some(fault) = feedback.fault {
            anyhow::bail!("{}: {fault}", setup.name);
        }
    }
    Ok(())
}

fn run_controller_setups<P: Plant>(
    mut setups: Vec<ArmSetup<P>>,
    socket_path: &Path,
    instance: uuid::Uuid,
    shutdown_requested: &AtomicBool,
    verify_torque: bool,
    bus_timing: impl Fn(&P) -> Option<&BusTiming>,
) -> anyhow::Result<()> {
    verify_controller_start(&mut setups, verify_torque)?;
    let socket =
        UnixStream::connect(socket_path).context("Cannot connect to the supervising station")?;
    let stop_socket = socket.try_clone()?;
    let arms: BTreeMap<String, LinkHandle> = setups
        .iter()
        .filter_map(|setup| {
            setup
                .link
                .as_ref()
                .map(|link| (setup.name.clone(), link.handle()))
        })
        .collect();
    let shutting_down = Arc::new(AtomicBool::new(false));
    let service_shutting_down = Arc::clone(&shutting_down);
    let service = std::thread::spawn(move || {
        let result = hebbian_robot_use_server::controller_service::serve_controller(
            socket,
            hebbian_robot_use_server::controller_transport::ControllerInstanceId(
                *instance.as_bytes(),
            ),
            &arms,
        );
        if !service_shutting_down.load(Ordering::Acquire) {
            eprintln!("Station connection lost: {result:?}");
            // The guard, when present, takes over after the controller exits.
            // Do not run the normal return-to-rest shutdown without the station.
            std::process::exit(1);
        }
    });
    let reports = run_arms(&mut setups, None, shutdown_requested);
    shutting_down.store(true, Ordering::Release);
    let _ = stop_socket.shutdown(std::net::Shutdown::Both);
    let _ = service.join();
    close_links(&mut setups);
    for (setup, (name, report)) in setups.iter().zip(&reports) {
        print_timing(name, report, bus_timing(&setup.plant));
    }
    Ok(())
}

/// Run a controller as a station child; it never serves HTTP itself.
pub(crate) fn run_controller(
    arms: &[ConfiguredArm],
    station_socket: &Path,
    instance: uuid::Uuid,
    socket_directory: Option<&Path>,
    plant_lag_seconds: f64,
    hardware: bool,
) -> anyhow::Result<()> {
    ensure!(
        plant_lag_seconds.is_finite() && plant_lag_seconds > 0.0,
        "--plant-lag-seconds must be positive"
    );
    let shutdown_requested = shutdown_flag()?;
    let control = Arc::new(Lease::new(LEASE));
    let simulated = arms.iter().any(|arm| arm.config.sim_plant.is_some());
    if simulated {
        ensure!(
            !hardware && arms.iter().all(|arm| arm.config.sim_plant.is_some()),
            "Controller arms must share a plant type"
        );
        let directory =
            socket_directory.context("Simulator controllers need a socket directory")?;
        let setups = simulator_setups(arms, directory, true, &shutdown_requested, &control)?;
        run_controller_setups(
            setups,
            station_socket,
            instance,
            &shutdown_requested,
            false,
            |_| None,
        )
    } else if hardware {
        let setups = hardware_setups(arms, true, &control)?;
        run_controller_setups(
            setups,
            station_socket,
            instance,
            &shutdown_requested,
            true,
            |plant| Some(plant.inner().bus().timing()),
        )
    } else {
        let setups = lag_setups(arms, plant_lag_seconds, true, &control)?;
        run_controller_setups(
            setups,
            station_socket,
            instance,
            &shutdown_requested,
            false,
            |_| None,
        )
    }
}

pub(crate) fn run_loop(
    arms: &[ConfiguredArm],
    seconds: Option<f64>,
    socket_directory: Option<&Path>,
    plant_lag_seconds: f64,
    hardware: bool,
    http_address: Option<&str>,
    web: WebSettings,
) -> anyhow::Result<()> {
    let run_for = seconds
        .map(|seconds| {
            Duration::try_from_secs_f64(seconds).context("--seconds must be a non-negative number")
        })
        .transpose()?;
    ensure!(
        plant_lag_seconds.is_finite() && plant_lag_seconds > 0.0,
        "--plant-lag-seconds must be positive"
    );
    let mut names: Vec<&str> = arms.iter().map(|arm| arm.name.as_str()).collect();
    names.sort_unstable();
    ensure!(
        names.windows(2).all(|pair| pair[0] != pair[1]),
        "Each --arm needs its own name"
    );
    let shutdown_requested = shutdown_flag()?;
    // One lease for every arm of this executor.
    let control = Arc::new(Lease::new(LEASE));
    let configs = arms.iter().map(|arm| &arm.config).collect::<Vec<_>>();
    let simulator_configs: Vec<bool> = configs
        .iter()
        .map(|config| config.sim_plant.is_some())
        .collect();
    // Started before the loop and closed after it, on its own threads.
    let serving = http_address
        .map(|address| {
            let directories = station_api::trace_directories(
                configs
                    .iter()
                    .map(|config| Path::new(config.trace_directory.as_str())),
            );
            let mut mounted = None;
            let live_run = Arc::new(LiveRunStore::default());
            let server = HttpServer::start(
                address,
                directories,
                station_api::default_viewer_directory(),
                live_run.clone(),
                |runtime, bound| {
                    let (routes, mounted_routes) = web::mount(runtime, bound, web);
                    mounted = Some(mounted_routes);
                    Ok(routes.merge(run_telemetry::routes(live_run)).merge(
                        controller_supervisor::router(
                            controller_supervisor::ControllerSupervisor::unmanaged(),
                        ),
                    ))
                },
            )?;
            let mounted = mounted.context("The station routes were not mounted")?;
            eprintln!("HTTP on http://{}", server.local_address());
            anyhow::Ok(Serving { server, mounted })
        })
        .transpose()?;
    if simulator_configs.contains(&true) {
        ensure!(
            simulator_configs.iter().all(|simulated| *simulated) && !hardware,
            "Arms with a sim_plant section cannot share an executor with arms on other plants"
        );
        let directory = socket_directory
            .context("Arms on a simulator need --socket-directory for their plant sockets")?;
        let setups = simulator_setups(
            arms,
            directory,
            serving.is_some(),
            &shutdown_requested,
            &control,
        )?;
        run_and_report(
            setups,
            run_for,
            &shutdown_requested,
            serving.as_ref(),
            |_| None,
        );
    } else if hardware {
        // Every arm starts disabled; the station enables each (preflight, then
        // torque on holding the present pose), or a run without a station enables
        // all at once. Torque stays on when the loop ends: the servos keep holding
        // their last goal, and `disable` is how an arm is let go.
        let setups = hardware_setups(arms, serving.is_some(), &control)?;
        run_and_report(
            setups,
            run_for,
            &shutdown_requested,
            serving.as_ref(),
            |plant| Some(plant.inner().bus().timing()),
        );
    } else {
        let setups = lag_setups(arms, plant_lag_seconds, serving.is_some(), &control)?;
        run_and_report(
            setups,
            run_for,
            &shutdown_requested,
            serving.as_ref(),
            |_| None,
        );
    }
    if let Some(serving) = serving {
        serving.close();
    }
    Ok(())
}

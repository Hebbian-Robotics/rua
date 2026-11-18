use std::path::{Path, PathBuf};

use anyhow::Context;
use hebbian_robot_use_server::config::{ExecutorConfig, SimPlantConfig};
use hebbian_robot_use_server::profile::{self, Driver, JointLimitSource};
use hebbian_station::web::WebSettings;
use station_access::new_room_name;

use crate::media::{MediaPlan, MediaSupervisor, SIMULATOR_CAMERA_SOCKET};
use crate::run::{ConfiguredArm, run_supervised};

const LAG_PLANT_SECONDS: f64 = 0.05;

use crate::commands::ServeArguments;

enum PlantKind {
    Servos,
    Simulator,
    Lag,
}

fn plant_kind(driver: &Driver, simulate: bool) -> PlantKind {
    match (simulate, driver) {
        (true, _) => PlantKind::Lag,
        (false, Driver::Feetech(_)) => PlantKind::Servos,
        (false, Driver::Simulator) => PlantKind::Simulator,
    }
}

struct TemporaryDirectory(PathBuf);

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn work_directory(
    requested: Option<PathBuf>,
) -> anyhow::Result<(PathBuf, Option<TemporaryDirectory>)> {
    if let Some(directory) = requested {
        std::fs::create_dir_all(&directory)
            .with_context(|| format!("Cannot create {}", directory.display()))?;
        return Ok((directory, None));
    }
    // CONSTRAINT: Unix socket paths are limited to about 100 bytes, and a platform's temporary
    // directory (macOS's `$TMPDIR`) can use most of that; `/tmp` is short everywhere.
    let directory = PathBuf::from("/tmp").join(format!("hebbian-{}", std::process::id()));
    std::fs::create_dir_all(&directory)
        .with_context(|| format!("Cannot create {}", directory.display()))?;
    Ok((directory.clone(), Some(TemporaryDirectory(directory))))
}

fn write_config(path: &Path, config: &ExecutorConfig) -> anyhow::Result<()> {
    std::fs::write(path, serde_json::to_string(config)?)
        .with_context(|| format!("Cannot write {}", path.display()))
}

pub(crate) fn serve(arguments: ServeArguments, mut web: WebSettings) -> anyhow::Result<()> {
    web.media_address = Some(arguments.media_address);
    let profile_directory = match &arguments.profile_directory {
        Some(directory) => directory.clone(),
        None => profile::default_profile_directory()?,
    };
    let robot_profile = arguments
        .selection
        .load_profile(&arguments.profile, &profile_directory)?;
    let kind = plant_kind(&robot_profile.driver, arguments.simulate);
    let arm_names = if arguments.arms.is_empty() {
        robot_profile.arms.clone()
    } else {
        arguments.arms
    };
    let (directory, _removed_at_exit) = work_directory(arguments.work_directory)?;
    let limit_source = match kind {
        PlantKind::Servos => JointLimitSource::Calibration,
        PlantKind::Simulator | PlantKind::Lag => JointLimitSource::Urdf,
    };
    let mut arms: Vec<ConfiguredArm> = Vec::new();
    for arm in arm_names {
        let mut config = profile::executor_config_input(
            &robot_profile,
            &arm,
            limit_source,
            &profile::lerobot_calibration_root(),
        )?;
        if matches!(kind, PlantKind::Simulator) {
            config.sim_plant = Some(SimPlantConfig {
                connect_timeout_seconds: arguments.simulator_connect_seconds,
                reply_timeout_seconds: arguments.simulator_reply_seconds,
                real_time: true,
            });
        }
        if let Some(trace_directory) = &arguments.trace_directory {
            config.trace_directory = trace_directory.to_string_lossy().into_owned();
        }
        let config = ExecutorConfig::try_from(config)
            .with_context(|| format!("Invalid config for arm {arm}"))?;
        let config_path = directory.join(format!("{arm}.json"));
        write_config(&config_path, &config)?;
        let guard_config_path = Some(config_path);
        arms.push(ConfiguredArm {
            name: arm,
            config,
            guard_config_path,
        });
    }
    if matches!(kind, PlantKind::Simulator) {
        let names: Vec<&str> = arms.iter().map(|arm| arm.name.as_str()).collect();
        eprintln!(
            "Plant sockets: {}/<arm>.plant.sock for {}",
            directory.display(),
            names.join(", ")
        );
    }
    // INVARIANT: The station and the media programs must name the same room.
    let room = web.room.get_or_insert_with(new_room_name).clone();
    let mut cameras = robot_profile.cameras.clone();
    for (name, device) in arguments.cameras {
        match device {
            Some(device) => cameras.insert(name, device),
            None => cameras.remove(&name),
        };
    }
    let _media = MediaSupervisor::start(
        &MediaPlan {
            executable: web.media_executable.clone(),
            address: arguments.media_address,
            room,
            station_url: format!("http://{}", arguments.http_address),
            cameras,
            simulator_camera_socket: matches!(kind, PlantKind::Simulator)
                .then(|| directory.join(SIMULATOR_CAMERA_SOCKET)),
        },
        &web.media_status,
    );
    run_supervised(
        &arms,
        &directory,
        LAG_PLANT_SECONDS,
        matches!(kind, PlantKind::Servos),
        &arguments.http_address,
        web,
    )
}

use std::fs::{DirBuilder, File, OpenOptions, Permissions};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use anyhow::{Context, ensure};
use hebbian_robot_use_core::config::{ControllerConfig, SimPlantConfig};
use hebbian_robot_use_core::profile::{self, Driver, JointLimitSource};
use hebbian_robot_use_server::controller_process::PlantSelection;
use hebbian_robot_use_server::web::WebSettings;
use station_access::new_room_name;

use crate::media::{MediaPlan, SIMULATOR_CAMERA_SOCKET};
use crate::run::{ConfiguredArm, DEFAULT_STAND_IN_LAG_SECONDS, run_supervised};

use crate::commands::ServeArguments;

const SIMULATOR_ARMS_FILE: &str = ".simulator-arms.json";

enum WorkDirectory {
    Temporary(tempfile::TempDir),
    Requested { path: PathBuf, lock: File },
}

impl WorkDirectory {
    fn open(requested: Option<PathBuf>) -> anyhow::Result<Self> {
        let Some(path) = requested else {
            // CONSTRAINT: A platform's temporary directory can use most of the Unix socket path limit.
            let directory = tempfile::Builder::new()
                .prefix("hebbian-")
                .permissions(Permissions::from_mode(0o700))
                .tempdir_in("/tmp")
                .context("Cannot create a station work directory in /tmp")?;
            return Ok(Self::Temporary(directory));
        };
        DirBuilder::new()
            .mode(0o700)
            .recursive(true)
            .create(&path)
            .with_context(|| format!("Cannot create work directory {}", path.display()))?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(nix::libc::O_NOFOLLOW)
            .open(path.join(".station-lock"))
            .with_context(|| format!("Cannot open the station lock in {}", path.display()))?;
        lock.try_lock()
            .with_context(|| format!("Cannot lock work directory {}", path.display()))?;
        Ok(Self::Requested { path, lock })
    }

    fn path(&self) -> &Path {
        match self {
            Self::Temporary(directory) => directory.path(),
            Self::Requested { path, .. } => path,
        }
    }
}

impl Drop for WorkDirectory {
    fn drop(&mut self) {
        if let Self::Requested { path, lock } = self {
            let _ = std::fs::remove_file(path.join(SIMULATOR_ARMS_FILE));
            // INVARIANT: Release immediately even if a child has forked and has yet to exec.
            let _ = lock.unlock();
        }
    }
}

fn write_config(path: &Path, config: &ControllerConfig) -> anyhow::Result<()> {
    std::fs::write(path, serde_json::to_string(config)?)
        .with_context(|| format!("Cannot write {}", path.display()))
}

fn clear_simulator_arms(directory: &Path) -> anyhow::Result<()> {
    match std::fs::remove_file(directory.join(SIMULATOR_ARMS_FILE)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).context("Cannot clear the previous simulator arm list"),
    }
}

fn publish_simulator_arms(directory: &Path, arms: &[ConfiguredArm]) -> anyhow::Result<()> {
    let names: Vec<&str> = arms.iter().map(|arm| arm.name.as_str()).collect();
    let mut manifest = tempfile::NamedTempFile::new_in(directory)?;
    serde_json::to_writer(&mut manifest, &names)?;
    manifest.persist(directory.join(SIMULATOR_ARMS_FILE))?;
    tracing::info!(
        socket_directory = %directory.display(),
        arms = ?names,
        "Simulator plant sockets are ready"
    );
    Ok(())
}

fn select_arms(profile_arms: &[String], requested: Vec<String>) -> anyhow::Result<Vec<String>> {
    if requested.is_empty() {
        return Ok(profile_arms.to_vec());
    }
    profile::validate_arm_names(requested.iter().map(String::as_str))?;
    for arm in &requested {
        ensure!(
            profile_arms.contains(arm),
            "Unknown arm {arm:?} in this profile"
        );
    }
    Ok(requested)
}

pub(crate) fn serve(arguments: ServeArguments, mut web: WebSettings) -> anyhow::Result<()> {
    web.media_address = Some(arguments.media_address);
    let profile_directory = arguments
        .profile_directory
        .map_or_else(profile::default_profile_directory, Ok)?;
    let robot_profile = arguments.selection.load_profile(
        &arguments.profile,
        &profile_directory,
        arguments.no_physics_stand_in,
    )?;
    let arm_names = select_arms(&robot_profile.arms, arguments.arms)?;
    let work_directory = WorkDirectory::open(arguments.work_directory)?;
    let directory = work_directory.path();
    clear_simulator_arms(directory)?;
    let plant = match (arguments.no_physics_stand_in, &robot_profile.driver) {
        (false, Driver::Feetech(_)) => PlantSelection::Servos,
        (false, Driver::Simulator) => PlantSelection::Simulator {
            socket_directory: directory.to_path_buf(),
        },
        (true, _) => PlantSelection::NoPhysicsStandIn {
            lag_seconds: DEFAULT_STAND_IN_LAG_SECONDS,
        },
    };
    let limit_source = if matches!(plant, PlantSelection::Servos) {
        JointLimitSource::Calibration
    } else {
        JointLimitSource::Urdf
    };
    // WHY: The simulator reads each arm's joint stops from the config written below, so a
    // simulated arm and the station's limits both match the mirrored arm.
    let mirrored_arm_limits = if matches!(plant, PlantSelection::Servos) {
        None
    } else {
        arguments.selection.mirrored_arm_joint_limits(
            &robot_profile,
            &profile_directory,
            &directory.join("calibration"),
        )?
    };
    let mut arms: Vec<ConfiguredArm> = Vec::new();
    for arm in arm_names {
        let mut config = profile::controller_config_input(
            &robot_profile,
            &arm,
            limit_source,
            &profile::lerobot_calibration_root(),
        )?;
        if let Some(limits) = &mirrored_arm_limits {
            config.lower_limits = limits.iter().map(|(lower, _)| *lower).collect();
            config.upper_limits = limits.iter().map(|(_, upper)| *upper).collect();
        }
        if matches!(plant, PlantSelection::Simulator { .. }) {
            config.sim_plant = Some(SimPlantConfig {
                connect_timeout_seconds: arguments.simulator_connect_seconds,
                reply_timeout_seconds: arguments.simulator_reply_seconds,
                real_time: true,
            });
        }
        if let Some(trace_directory) = &arguments.trace_directory {
            config.trace_directory = trace_directory.to_string_lossy().into_owned();
        }
        let config = ControllerConfig::try_from(config)
            .with_context(|| format!("Invalid config for arm {arm}"))?;
        let config_path = directory.join(format!("{arm}.json"));
        write_config(&config_path, &config)?;
        arms.push(ConfiguredArm {
            name: arm.parse()?,
            config,
            config_path,
        });
    }
    if matches!(plant, PlantSelection::Simulator { .. }) {
        publish_simulator_arms(directory, &arms)?;
    }
    // INVARIANT: The station and the media service must name the same room.
    let room = web.room.get_or_insert_with(new_room_name).clone();
    let mut cameras = robot_profile.cameras;
    let mut usb_cameras = if matches!(plant, PlantSelection::Servos) {
        robot_profile.usb_cameras
    } else {
        std::collections::BTreeMap::new()
    };
    usb_cameras.retain(|name, _| !cameras.contains_key(name));
    for (name, device) in arguments.cameras {
        usb_cameras.remove(&name);
        match device {
            Some(device) => cameras.insert(name, device),
            None => cameras.remove(&name),
        };
    }
    let media = MediaPlan {
        executable: web.media_executable.clone(),
        address: arguments.media_address,
        room,
        cameras,
        usb_cameras,
        simulator_camera_socket: matches!(plant, PlantSelection::Simulator { .. })
            .then(|| directory.join(SIMULATOR_CAMERA_SOCKET)),
    };
    run_supervised(&arms, directory, plant, &arguments.http_address, web, media)
}

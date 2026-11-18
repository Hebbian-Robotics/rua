#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    reason = "This integration-test target may abort on failed setup or unexpected outcomes."
)]
#![cfg(unix)]

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use hebbian_robot_use_server::config::{ExecutorConfig, GuardConfig};
use hebbian_robot_use_server::guard::{GuardedArm, guard};
use hebbian_robot_use_server::plant::Feedback;

const ROLE_VARIABLE: &str = "HEBBIAN_GUARD_PROCESS_TEST_ROLE";
const DIRECTORY_VARIABLE: &str = "HEBBIAN_GUARD_PROCESS_TEST_DIRECTORY";
const EXECUTOR_PID_VARIABLE: &str = "HEBBIAN_GUARD_PROCESS_TEST_EXECUTOR";
const TEST_NAME: &str = "simulated_guard_takes_over_after_controller_stalls";

struct LockedBus {
    _lock: File,
    directory: PathBuf,
}

impl LockedBus {
    fn open(directory: &Path) -> std::io::Result<Self> {
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(directory.join("bus.lock"))?;
        // SAFETY: `lock` is a live descriptor, and flock does not retain pointers.
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(Self {
            _lock: lock,
            directory: directory.to_owned(),
        })
    }
}

impl GuardedArm for LockedBus {
    fn read(&mut self) -> anyhow::Result<(Feedback, Vec<bool>)> {
        let powered = std::fs::read_to_string(self.directory.join("torque"))? == "on";
        Ok((
            Feedback {
                positions: vec![0.0; 2],
                velocities: vec![0.0; 2],
                efforts: vec![0.0; 2],
                temperatures_celsius: vec![],
                voltages_volts: vec![],
                environment_confirmations: vec![],
                gripper_angle: None,
                fault: None,
            },
            vec![powered; 2],
        ))
    }

    fn release(&mut self) -> anyhow::Result<()> {
        std::fs::write(self.directory.join("torque"), "off")?;
        Ok(())
    }
}

fn guard_configuration() -> ExecutorConfig {
    let config = ExecutorConfig::parse(r#"{
      "urdf":"unused.urdf","driven_joints":["a","b"],"surface_guard":null,
      "motion":{"acceleration_limit":20.0,"jerk_limit":200.0,"settled_error_radians":0.02,"settle_dwell_seconds":0.1,"settle_timeout_seconds":2.0},
      "joint_count":2,"frequency_hertz":100.0,"loop_deadline_seconds":0.1,
      "lower_limits":[-2.0,-2.0],"upper_limits":[2.0,2.0],"joint_effort_limits":[10.0,10.0],
      "tracking_following_lag_seconds":0.05,"tracking_static_margin_radians":1.5,"max_joint_speed":3.0,"default_joint_speed":1.5,
      "stream_goal_timeout_seconds":5.0,"joint_limit_recovery_tolerance_radians":0.0349,"jaw_closing_axis":"x"
    }"#).unwrap();
    let mut config = config.into_input();
    config.guard = Some(GuardConfig {
        heartbeat_timeout_seconds: 0.4,
        poll_seconds: 0.02,
        hold_limit_seconds: 1.0,
    });
    ExecutorConfig::try_from(config).unwrap()
}

fn spawn_role(role: &str, directory: &Path) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", TEST_NAME, "--nocapture"])
        .env(ROLE_VARIABLE, role)
        .env(DIRECTORY_VARIABLE, directory);
    command
}

fn run_guard(directory: &Path) {
    let executor_pid = std::env::var(EXECUTOR_PID_VARIABLE)
        .unwrap()
        .parse()
        .unwrap();
    let result = guard(
        &guard_configuration(),
        executor_pid,
        std::io::stdin(),
        || {
            let bus = LockedBus::open(directory)?;
            std::fs::write(directory.join("guard-acquired"), "yes")?;
            Ok(bus)
        },
    );
    assert!(result.is_ok(), "guard failed: {result:?}");
}

fn run_controller(directory: &Path) {
    let _bus = LockedBus::open(directory).unwrap();
    std::fs::write(directory.join("torque"), "on").unwrap();
    let mut guard_process = spawn_role("guard", directory)
        .env(EXECUTOR_PID_VARIABLE, std::process::id().to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let stdout = guard_process.stdout.take().unwrap();
    let mut reader = BufReader::new(stdout);
    loop {
        let mut line = String::new();
        assert_ne!(
            reader.read_line(&mut line).unwrap(),
            0,
            "guard exited before ready"
        );
        if line.trim() == "ready" {
            break;
        }
    }
    println!("controller-ready");
    std::io::stdout().flush().unwrap();
    let mut heartbeats = guard_process.stdin.take().unwrap();
    loop {
        if heartbeats.write_all(b".").is_err() {
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    let _ = guard_process.wait();
}

struct CleanupChild(Child);
impl Drop for CleanupChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn wait_until(mut predicate: impl FnMut() -> bool, message: &str) {
    let deadline = Instant::now() + Duration::from_secs(8);
    while !predicate() {
        assert!(Instant::now() < deadline, "{message}");
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn simulated_guard_takes_over_after_controller_stalls() {
    if let Ok(role) = std::env::var(ROLE_VARIABLE) {
        let directory = PathBuf::from(std::env::var(DIRECTORY_VARIABLE).unwrap());
        match role.as_str() {
            "controller" => run_controller(&directory),
            "guard" => run_guard(&directory),
            _ => panic!("unknown role"),
        }
        return;
    }
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let directory =
        std::env::temp_dir().join(format!("hebbian-guard-{}-{unique}", std::process::id()));
    std::fs::create_dir(&directory).unwrap();
    let child = spawn_role("controller", &directory)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut controller = CleanupChild(child);
    let stdout = controller.0.stdout.take().unwrap();
    let stderr = controller.0.stderr.take().unwrap();
    let (ready, readiness) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if line == "controller-ready" {
                let _ = ready.send(());
                break;
            }
        }
    });
    let (closed, report_closed) = mpsc::channel();
    thread::spawn(move || {
        let mut reports = String::new();
        let _ = BufReader::new(stderr).read_to_string(&mut reports);
        let _ = closed.send(reports);
    });
    readiness
        .recv_timeout(Duration::from_secs(8))
        .expect("controller and guard started");
    assert!(
        LockedBus::open(&directory).is_err(),
        "controller owns the bus"
    );
    let controller_pid = i32::try_from(controller.0.id()).unwrap();
    // SAFETY: this pid is the simulated controller spawned by this test.
    assert_eq!(unsafe { libc::kill(controller_pid, libc::SIGSTOP) }, 0);
    wait_until(
        || directory.join("guard-acquired").exists(),
        "guard did not take over the bus",
    );
    let status = controller
        .0
        .try_wait()
        .unwrap()
        .expect("guard stopped the stalled controller");
    assert!(!status.success());
    assert_eq!(
        std::fs::read_to_string(directory.join("torque")).unwrap(),
        "on"
    );
    assert!(
        LockedBus::open(&directory).is_err(),
        "guard exclusively owns the bus while holding"
    );
    assert!(
        report_closed.try_recv().is_err(),
        "guard retains the reporting pipe while supervising"
    );
    let reports = report_closed
        .recv_timeout(Duration::from_secs(8))
        .expect("guard did not release the bus");
    assert!(reports.contains("Arm released"), "{reports}");
    assert_eq!(
        std::fs::read_to_string(directory.join("torque")).unwrap(),
        "off"
    );
    assert!(
        LockedBus::open(&directory).is_ok(),
        "the bus is available after guard release"
    );
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn guard_releases_the_arm_even_when_the_stations_report_pipe_disappears() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "hebbian-guard-pipe-{}-{unique}",
        std::process::id()
    ));
    std::fs::create_dir(&directory).unwrap();
    let child = spawn_role("controller", &directory)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut controller = CleanupChild(child);
    let stdout = controller.0.stdout.take().unwrap();
    let (ready, readiness) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if line == "controller-ready" {
                let _ = ready.send(());
                break;
            }
        }
    });
    readiness
        .recv_timeout(Duration::from_secs(8))
        .expect("controller and guard started");
    drop(controller.0.stderr.take());
    let controller_pid = i32::try_from(controller.0.id()).unwrap();
    // SAFETY: this pid is the simulated controller spawned by this test.
    assert_eq!(unsafe { libc::kill(controller_pid, libc::SIGSTOP) }, 0);
    wait_until(
        || directory.join("guard-acquired").exists(),
        "guard did not acquire the simulated bus",
    );
    wait_until(
        || std::fs::read_to_string(directory.join("torque")).is_ok_and(|torque| torque == "off"),
        "guard did not release torque after its report pipe closed",
    );
    wait_until(
        || LockedBus::open(&directory).is_ok(),
        "guard did not release the simulated bus",
    );
    assert!(!controller.0.try_wait().unwrap().unwrap().success());
    std::fs::remove_dir_all(directory).unwrap();
}

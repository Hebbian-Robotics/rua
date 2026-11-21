#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    reason = "This integration-test target may abort on failed setup or unexpected outcomes."
)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use hebbian_contracts::{ArmMode, Command as ArmCommand, Event};
use hebbian_robot_use_core::controller_transport::{
    ControllerAnswer, ControllerMessage, ControllerRequest, ControllerRequestId,
    ControllerTransport,
};
use hebbian_robot_use_server::controller_process::{
    ControllerLaunch, ControllerProcess, PlantSelection,
};

struct ControllerChild(ControllerProcess);

impl Drop for ControllerChild {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let pid = nix::unistd::Pid::from_raw(i32::try_from(self.0.process_id()).unwrap());
            let _ = nix::sys::signal::kill(pid, nix::sys::signal::Signal::SIGKILL);
        }
    }
}

fn write_simulated_config(root: &Path, directory: &Path) -> PathBuf {
    let executable = env!("CARGO_BIN_EXE_hebbian-robot-use-server");
    let config_output = Command::new(executable)
        .current_dir(root)
        .args([
            "config",
            "--profile",
            "so101-sim",
            "--arm",
            "arm",
            "--urdf-limits",
            "--profile-directory",
        ])
        .arg(root.join("profiles"))
        .output()
        .unwrap();
    assert!(
        config_output.status.success(),
        "{}",
        String::from_utf8_lossy(&config_output.stderr)
    );
    let mut config: serde_json::Value = serde_json::from_slice(&config_output.stdout).unwrap();
    config["trace_directory"] = directory.join("traces").to_str().unwrap().into();
    let config_path = directory.join("arm.json");
    std::fs::write(&config_path, serde_json::to_vec(&config).unwrap()).unwrap();
    config_path
}

#[test]
fn the_diagnostic_rejects_hardware_without_servo_configuration() {
    let executable = env!("CARGO_BIN_EXE_hebbian-robot-use-server");
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let temporary = tempfile::tempdir().unwrap();
    let config_path = write_simulated_config(root, temporary.path());
    let output = Command::new(executable)
        .args(["run", "--arm"])
        .arg(format!("arm={}", config_path.display()))
        .arg("--hardware")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success());
    assert!(stderr.contains("needs a feetech section"), "{stderr}");
}

#[test]
fn simulated_controller_runs_in_a_child_and_exits_when_station_disconnects() {
    let executable = env!("CARGO_BIN_EXE_hebbian-robot-use-server");
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let temporary = tempfile::tempdir().unwrap();
    let config_path = write_simulated_config(root, temporary.path());
    let child = ControllerProcess::start(&ControllerLaunch {
        executable: executable.into(),
        arms: vec![("arm".into(), config_path)],
        work_directory: temporary.path().into(),
        plant: PlantSelection::NoPhysicsStandIn { lag_seconds: 0.05 },
    })
    .unwrap();
    let mut child = ControllerChild(child);
    let deadline = Instant::now() + Duration::from_secs(15);
    let socket = loop {
        match child.0.try_accept() {
            Ok(Some(socket)) => break socket,
            Ok(None) => {
                assert!(
                    child.0.try_wait().unwrap().is_none(),
                    "controller exited before connecting"
                );
                assert!(Instant::now() < deadline, "controller did not connect");
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("accept controller: {error:#}"),
        }
    };
    socket
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let mut station = ControllerTransport::new(socket, child.0.instance());
    loop {
        if matches!(station.receive().unwrap(), ControllerMessage::Event { event, .. } if matches!(*event, Event::State(ref sample) if sample.mode == ArmMode::Disabled))
        {
            break;
        }
        assert!(Instant::now() < deadline, "no disabled feedback");
    }
    station
        .send(ControllerMessage::Request {
            request_id: ControllerRequestId(0),
            arm: "arm".into(),
            request: ControllerRequest::Submit {
                command: ArmCommand::Enable,
                trace_context: None,
            },
        })
        .unwrap();
    let mut accepted = false;
    let mut enabled = false;
    while !accepted || !enabled {
        match station.receive().unwrap() {
            ControllerMessage::Answer {
                request_id: ControllerRequestId(0),
                answer: ControllerAnswer::Submitted(_),
            } => accepted = true,
            ControllerMessage::Event { event, .. } if matches!(*event, Event::State(ref sample) if sample.mode == ArmMode::Holding) =>
            {
                enabled = true;
            }
            _ => {}
        }
        assert!(Instant::now() < deadline, "controller did not enable");
    }
    drop(station);
    loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            assert!(!status.success());
            break;
        }
        assert!(
            Instant::now() < deadline,
            "controller survived its station connection"
        );
        thread::sleep(Duration::from_millis(10));
    }
    while !child.0.report_pipe_closed() {
        assert!(
            Instant::now() < deadline,
            "controller report pipe stayed open"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

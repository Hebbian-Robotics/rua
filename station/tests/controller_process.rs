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
    ControllerAnswer, ControllerEvent, ControllerMessage, ControllerRequest, ControllerRequestId,
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
        if matches!(station.receive().unwrap(), ControllerMessage::Event(ControllerEvent { event, .. }) if matches!(*event, Event::State(ref sample) if sample.mode == ArmMode::Disabled))
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
            ControllerMessage::Event(ControllerEvent { event, .. }) if matches!(*event, Event::State(ref sample) if sample.mode == ArmMode::Holding) =>
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

#[tokio::test]
async fn controller_exits_when_station_dies_during_the_simulator_handshake()
-> Result<(), Box<dyn std::error::Error>> {
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

    let executable = env!("CARGO_BIN_EXE_hebbian-robot-use-server");
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let temporary = tempfile::tempdir()?;
    let config_path = write_simulated_config(root, temporary.path());
    let mut config: serde_json::Value = serde_json::from_slice(&std::fs::read(&config_path)?)?;
    config["sim_plant"] = serde_json::json!({
        "connect_timeout_seconds": 60,
        "reply_timeout_seconds": 60,
    });
    std::fs::write(&config_path, serde_json::to_vec(&config)?)?;
    let mut station = tokio::process::Command::new("sh")
        .args([
            "-c",
            r#""$1" controller --station-pid "$$" --station-socket "$2" --instance "$3" --arm "$4" --socket-directory "$5" & echo $!; wait"#,
            "station-supervisor",
        ])
        .arg(executable)
        .arg(temporary.path().join("station.sock"))
        .arg(uuid::Uuid::new_v4().to_string())
        .arg(format!("arm={}", config_path.display()))
        .arg(temporary.path())
        .stdout(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let mut output = BufReader::new(station.stdout.take().ok_or("no stdout")?);
    let mut line = String::new();
    let deadline = Duration::from_secs(5);
    tokio::time::timeout(deadline, output.read_line(&mut line)).await??;
    let controller_pid = line.trim().parse::<i32>()?;
    let result = tokio::time::timeout(deadline, async {
        let plant_socket = temporary.path().join("arm.plant.sock");
        let mut simulator = loop {
            if let Ok(socket) = tokio::net::UnixStream::connect(&plant_socket).await {
                break socket;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        };
        simulator.write_all(b"{\"protocol_version\":").await?;
        // WHY: Sending more than the socket buffer establishes that the controller
        // has begun reading the hello, which remains incomplete.
        simulator.write_all(&vec![b' '; 512 * 1024]).await?;
        station.kill().await?;
        // WHY: The controller owns the inherited stdout pipe. EOF
        // observes exit independently of how soon the container reaps the orphan.
        let mut remainder = Vec::new();
        output.read_to_end(&mut remainder).await?;
        drop(simulator);
        Ok::<(), std::io::Error>(())
    })
    .await;
    if !matches!(result, Ok(Ok(()))) {
        let _ = nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(controller_pid),
            nix::sys::signal::Signal::SIGKILL,
        );
    }
    result??;
    Ok(())
}

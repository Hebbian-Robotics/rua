// INVARIANT: Read step replies at the next cycle’s start so every arm sharing a scene can submit its step before the simulator advances the common physics.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::time::{Duration, Instant};

use anyhow::{Context, bail, ensure};
use hebbian_contracts::{SimHello, SimPlantConfig, SimPlantContract, SimState, SimStep};
use serde::Deserialize;

use crate::numeric::positive_clock_duration;
use crate::plant::{Feedback, Pacing, Plant, PlantWritePermit};

pub(crate) const PROTOCOL_VERSION: u32 = 1;

#[must_use]
#[expect(
    clippy::expect_used,
    reason = "Schemars schemas contain only JSON-compatible data and cannot fail JSON serialization."
)]
pub fn schema_json() -> String {
    let schema = schemars::schema_for!(SimPlantContract);
    let mut text = serde_json::to_string_pretty(&schema).expect("a schema serializes");
    text.push('\n');
    text
}

#[derive(Debug, Clone, Copy)]
pub struct SimTimeouts {
    pub connect: Duration,
    pub reply: Duration,
    pub real_time: bool,
}

impl TryFrom<&SimPlantConfig> for SimTimeouts {
    type Error = anyhow::Error;

    fn try_from(config: &SimPlantConfig) -> anyhow::Result<Self> {
        Ok(Self {
            connect: positive_clock_duration(
                config.connect_timeout_seconds,
                "Simulator connect timeout",
            )?,
            reply: positive_clock_duration(
                config.reply_timeout_seconds,
                "Simulator reply timeout",
            )?,
            real_time: config.real_time,
        })
    }
}

pub struct SocketPlant {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
    joint_count: usize,
    state: SimState,
    gripper_target: Option<f64>,
    powered: bool,
    awaiting_reply: bool,
    real_time: bool,
}

impl SocketPlant {
    pub fn accept(
        listener: &UnixListener,
        driven_joints: &[String],
        joint_count: usize,
        timeouts: SimTimeouts,
        should_stop: impl Fn() -> bool,
    ) -> anyhow::Result<Self> {
        listener.set_nonblocking(true)?;
        let started = Instant::now();
        let stream = loop {
            ensure!(
                !should_stop(),
                "Stopped while waiting for a simulator to connect"
            );
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    ensure!(
                        started.elapsed() < timeouts.connect,
                        "No simulator connected within {} s",
                        timeouts.connect.as_secs_f64()
                    );
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(error) => return Err(error).context("Accepting the simulator failed"),
            }
        };
        stream.set_nonblocking(false)?;
        stream.set_write_timeout(Some(timeouts.reply))?;
        let mut reader = BufReader::new(stream.try_clone()?);
        let hello = read_hello(&mut reader, timeouts.reply, should_stop)
            .context("The simulator's hello")?;
        stream.set_read_timeout(Some(timeouts.reply))?;
        ensure!(
            hello.protocol_version == PROTOCOL_VERSION,
            "The simulator speaks protocol {}, the controller {PROTOCOL_VERSION}",
            hello.protocol_version
        );
        ensure!(
            hello.joint_names == driven_joints,
            "The simulator serves joints {:?}, but this arm drives {driven_joints:?}",
            hello.joint_names
        );
        let has_gripper = driven_joints.len() == joint_count + 1;
        check_state(&hello.state, joint_count, has_gripper)?;
        Ok(Self {
            reader,
            writer: stream,
            joint_count,
            gripper_target: hello.state.gripper_angle,
            state: hello.state,
            powered: false,
            awaiting_reply: false,
            real_time: timeouts.real_time,
        })
    }

    fn send_step(&mut self, positions: &[f64], seconds: f64) -> anyhow::Result<()> {
        ensure!(
            !self.awaiting_reply,
            "The simulator has not answered the previous step"
        );
        let step = SimStep {
            positions: positions.to_vec(),
            gripper_angle: self.gripper_target,
            seconds,
            powered: self.powered,
        };
        let mut line = serde_json::to_string(&step)?;
        line.push('\n');
        self.writer
            .write_all(line.as_bytes())
            .context("The simulator is gone")?;
        self.awaiting_reply = true;
        Ok(())
    }
}

fn read_hello(
    reader: &mut BufReader<UnixStream>,
    timeout: Duration,
    should_stop: impl Fn() -> bool,
) -> anyhow::Result<SimHello> {
    let started = Instant::now();
    let mut line = Vec::new();
    loop {
        ensure!(
            !should_stop(),
            "Stopped while waiting for the simulator's hello"
        );
        let remaining = timeout.saturating_sub(started.elapsed());
        ensure!(
            !remaining.is_zero(),
            "The simulator did not answer within {} s",
            timeout.as_secs_f64()
        );
        // WHY: Retain partial bytes and check for station loss even when a peer
        // connects but never finishes sending its hello.
        reader
            .get_ref()
            .set_read_timeout(Some(remaining.min(Duration::from_millis(100))))?;
        let available = match reader.fill_buf() {
            Ok([]) => bail!("The simulator disconnected"),
            Ok(available) => available,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::Interrupted
                        | std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::TimedOut
                ) =>
            {
                continue;
            }
            Err(error) => return Err(error).context("Cannot read the simulator's hello"),
        };
        let newline = available.iter().position(|byte| *byte == b'\n');
        let consumed = newline.map_or(available.len(), |index| index + 1);
        line.extend_from_slice(&available[..consumed]);
        reader.consume(consumed);
        if newline.is_some() {
            return serde_json::from_slice(&line).context("Invalid simulator JSON");
        }
    }
}

fn check_state(state: &SimState, joint_count: usize, has_gripper: bool) -> anyhow::Result<()> {
    ensure!(
        state.positions.len() == joint_count
            && state.velocities.len() == joint_count
            && state.efforts.len() == joint_count,
        "The simulator reported {} positions, {} velocities and {} efforts for {joint_count} joints",
        state.positions.len(),
        state.velocities.len(),
        state.efforts.len()
    );
    ensure!(
        state.gripper_angle.is_some() == has_gripper,
        "The simulator's gripper does not match the arm's"
    );
    ensure!(
        state
            .positions
            .iter()
            .chain(&state.velocities)
            .chain(&state.efforts)
            .chain(&state.gripper_angle)
            .all(|value| value.is_finite()),
        "The simulator reported a value that is not finite"
    );
    Ok(())
}

fn read_line<T: for<'de> Deserialize<'de>>(
    reader: &mut BufReader<UnixStream>,
) -> anyhow::Result<T> {
    let mut line = String::new();
    match reader.read_line(&mut line) {
        Ok(0) => bail!("The simulator disconnected"),
        Ok(_) => serde_json::from_str(&line).context("The simulator sent an invalid message"),
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
            ) =>
        {
            bail!("The simulator did not answer in time")
        }
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::BrokenPipe
                    | std::io::ErrorKind::UnexpectedEof
            ) =>
        {
            bail!("The simulator disconnected")
        }
        Err(error) => Err(error).context("Reading from the simulator failed"),
    }
}

impl Plant for SocketPlant {
    fn read_feedback_into(&mut self, feedback: &mut Feedback) -> anyhow::Result<()> {
        if self.awaiting_reply {
            self.state = read_line(&mut self.reader)?;
            self.awaiting_reply = false;
            check_state(&self.state, self.joint_count, self.gripper_target.is_some())?;
        }
        feedback.positions.clone_from(&self.state.positions);
        feedback.velocities.clone_from(&self.state.velocities);
        feedback.efforts.clone_from(&self.state.efforts);
        feedback.temperatures_celsius.clear();
        feedback.voltages_volts.clear();
        feedback.environment_confirmations.clear();
        feedback.gripper_angle = self.state.gripper_angle;
        feedback.fault = None;
        Ok(())
    }

    fn command(
        &mut self,
        _permit: &PlantWritePermit,
        positions: &[f64],
        seconds: f64,
    ) -> anyhow::Result<()> {
        self.send_step(positions, seconds)
    }

    fn command_gripper(&mut self, _permit: &PlantWritePermit, angle: f64) -> anyhow::Result<()> {
        ensure!(self.gripper_target.is_some(), "This arm has no gripper");
        self.gripper_target = Some(angle);
        Ok(())
    }

    fn preflight(&mut self) -> anyhow::Result<()> {
        Ok(())
    }

    fn enable(&mut self, _permit: &PlantWritePermit) -> anyhow::Result<()> {
        self.powered = true;
        Ok(())
    }

    fn release(&mut self, _permit: &PlantWritePermit) -> anyhow::Result<()> {
        self.powered = false;
        Ok(())
    }

    fn pacing(&self) -> Pacing {
        if self.real_time {
            Pacing::RepliesAtClockSpeed
        } else {
            Pacing::RepliesAsFastAsPossible
        }
    }

    fn advance(&mut self, _permit: &PlantWritePermit, seconds: f64) -> anyhow::Result<()> {
        // WHY: An arm whose loop has ended is not read any more; take its last reply
        // before the next step so the simulator is not left waiting.
        if self.awaiting_reply {
            self.read_feedback()?;
        }
        let rest = self.state.positions.clone();
        self.send_step(&rest, seconds)
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::net::UnixStream;

    use super::*;

    fn joints() -> Vec<String> {
        vec!["a".to_owned(), "b".to_owned()]
    }

    fn state(position: f64) -> SimState {
        SimState {
            positions: vec![position; 2],
            velocities: vec![0.0; 2],
            efforts: vec![0.0; 2],
            gripper_angle: None,
        }
    }

    fn timeouts() -> SimTimeouts {
        SimTimeouts {
            connect: Duration::from_secs(2),
            reply: Duration::from_millis(300),
            real_time: true,
        }
    }

    fn listen(test_name: &str) -> (UnixListener, std::path::PathBuf) {
        let path = std::env::temp_dir().join(format!(
            "hebbian-sim-plant-{test_name}-{}.sock",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        (UnixListener::bind(&path).unwrap(), path)
    }

    fn say_hello(stream: &mut UnixStream, names: Vec<String>) {
        let hello = SimHello {
            protocol_version: PROTOCOL_VERSION,
            joint_names: names,
            state: state(0.25),
        };
        writeln!(stream, "{}", serde_json::to_string(&hello).unwrap()).unwrap();
    }

    fn connected(test_name: &str) -> (SocketPlant, UnixStream, BufReader<UnixStream>) {
        let (listener, path) = listen(test_name);
        let mut sim = UnixStream::connect(&path).unwrap();
        say_hello(&mut sim, joints());
        let plant = SocketPlant::accept(&listener, &joints(), 2, timeouts(), || false).unwrap();
        let reader = BufReader::new(sim.try_clone().unwrap());
        (plant, sim, reader)
    }

    #[test]
    fn the_simulator_drives_each_step_and_the_reply_is_read_next_cycle() {
        let (mut plant, mut sim, mut sim_reader) = connected("steps");
        assert_eq!(plant.read_feedback().unwrap().positions, vec![0.25; 2]);
        plant.powered = true;
        plant.send_step(&[0.5, -0.5], 0.01).unwrap();
        let step: SimStep = read_line(&mut sim_reader).unwrap();
        assert_eq!(step.positions, vec![0.5, -0.5]);
        assert!(step.powered);
        assert!((step.seconds - 0.01).abs() < f64::EPSILON);
        writeln!(sim, "{}", serde_json::to_string(&state(0.3)).unwrap()).unwrap();
        assert_eq!(plant.read_feedback().unwrap().positions, vec![0.3; 2]);
    }

    #[test]
    fn two_steps_without_a_reply_between_are_refused() {
        let (mut plant, _sim, _sim_reader) = connected("double");
        plant.send_step(&[0.0, 0.0], 0.01).unwrap();
        assert!(plant.send_step(&[0.0, 0.0], 0.01).is_err());
    }

    #[test]
    fn a_silent_simulator_is_an_error_not_a_hang() {
        let (mut plant, _sim, _sim_reader) = connected("silent");
        plant.send_step(&[0.0, 0.0], 0.01).unwrap();
        let error = plant.read_feedback().err().unwrap();
        assert!(format!("{error:#}").contains("did not answer"));
    }

    #[test]
    fn a_disconnected_simulator_is_an_error() {
        let (mut plant, sim, sim_reader) = connected("gone");
        plant.send_step(&[0.0, 0.0], 0.01).unwrap();
        drop(sim);
        drop(sim_reader);
        let error = plant.read_feedback().err().unwrap();
        assert!(format!("{error:#}").contains("disconnected"));
    }

    #[test]
    fn a_simulator_serving_other_joints_is_refused_at_hello() {
        let (listener, path) = listen("names");
        let mut sim = UnixStream::connect(&path).unwrap();
        say_hello(&mut sim, vec!["x".to_owned(), "y".to_owned()]);
        let error = SocketPlant::accept(&listener, &joints(), 2, timeouts(), || false)
            .err()
            .unwrap();
        assert!(format!("{error:#}").contains("serves joints"));
    }

    #[test]
    fn waiting_for_a_simulator_ends_on_shutdown_or_timeout() {
        let (listener, _path) = listen("nobody");
        let quick = SimTimeouts {
            connect: Duration::from_millis(100),
            reply: Duration::from_millis(100),
            real_time: true,
        };
        let error = SocketPlant::accept(&listener, &joints(), 2, quick, || false)
            .err()
            .unwrap();
        assert!(format!("{error:#}").contains("No simulator connected"));
        let error = SocketPlant::accept(&listener, &joints(), 2, timeouts(), || true)
            .err()
            .unwrap();
        assert!(format!("{error:#}").contains("Stopped"));
    }
}

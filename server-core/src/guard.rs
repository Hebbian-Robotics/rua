#![allow(
    clippy::disallowed_types,
    clippy::disallowed_methods,
    reason = "The independent guard must read and release hardware after controller failure."
)]

// SAFETY: Start the separate guard before torque goes on. If heartbeats stop it takes over the bus and supervises the hold; faults, effort, drift or the hold deadline release the arm.

use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::{Duration, Instant};

use anyhow::{Context, bail};

use crate::config::{ControllerConfig, GuardTiming};
use crate::limits::{check_effort, max_joint_deviation};
use crate::plant::{Feedback, Plant, Reconnection};
use crate::so101::driver::{SerialBus, Servos};

const CONTROLLER_EXIT: Duration = Duration::from_secs(2);
const SUPERVISOR_START: Duration = Duration::from_secs(10);
const DETACH_WAIT: Duration = Duration::from_secs(3);
const HEARTBEAT_READ_BUFFER_BYTES: usize = 4096;

pub(crate) struct HoldSupervisor<'a> {
    config: &'a ControllerConfig,
    hold_limit: Duration,
    reference_positions: Vec<f64>,
    started_at: Instant,
}

impl<'a> HoldSupervisor<'a> {
    #[must_use]
    pub fn new(
        config: &'a ControllerConfig,
        timing: GuardTiming,
        reference_positions: Vec<f64>,
        started_at: Instant,
    ) -> Self {
        Self {
            config,
            hold_limit: timing.hold_limit,
            reference_positions,
            started_at,
        }
    }

    #[must_use]
    pub(crate) fn disable_reason(
        &self,
        feedback: &Feedback,
        torque_enabled: &[bool],
        now: Instant,
    ) -> Option<String> {
        if torque_enabled.iter().any(|enabled| !enabled) {
            return Some(format!(
                "Motor status during hold: torque {torque_enabled:?}"
            ));
        }
        if let Some(fault) = &feedback.fault {
            return Some(format!("Motor status during hold: {fault}"));
        }
        if check_effort(self.config, &feedback.efforts).is_err() {
            return Some(format!("Effort limit during hold: {:?}", feedback.efforts));
        }
        if max_joint_deviation(&feedback.positions, &self.reference_positions)
            > self.config.tracking_static_margin_radians
        {
            return Some("Arm moved during hold".to_owned());
        }
        if now.duration_since(self.started_at) > self.hold_limit {
            return Some(format!(
                "Controller silent for over {} s",
                self.hold_limit.as_secs_f64()
            ));
        }
        None
    }
}

pub trait GuardedArm {
    fn read(&mut self) -> anyhow::Result<(Feedback, Vec<bool>)>;
    fn release(&mut self) -> anyhow::Result<()>;
}

impl GuardedArm for Servos<SerialBus> {
    fn read(&mut self) -> anyhow::Result<(Feedback, Vec<bool>)> {
        let feedback = Plant::read_feedback(self)?;
        let torque = self.torque_enabled()?;
        Ok((feedback, torque))
    }

    fn release(&mut self) -> anyhow::Result<()> {
        Plant::release(self)
    }
}

struct GuardReading {
    feedback: Feedback,
    torque: Vec<bool>,
}

impl GuardReading {
    fn read(arm: &mut impl GuardedArm, config: &ControllerConfig) -> anyhow::Result<Self> {
        let (feedback, torque) = arm.read()?;
        feedback
            .measured_positions(config)
            .context("Guard received malformed feedback; releasing the arm")?;
        anyhow::ensure!(
            !torque.is_empty(),
            "Motor torque state is unavailable; the arm may still be powered"
        );
        Ok(Self { feedback, torque })
    }
}

fn release_and_confirm<A: GuardedArm>(arm: &mut A) -> anyhow::Result<()> {
    arm.release()
        .context("The guard could not release the arm; it may still be powered")?;
    let (_, torque) = arm
        .read()
        .context("Could not confirm motor torque after release; the arm may still be powered")?;
    anyhow::ensure!(
        !torque.is_empty() && torque.iter().all(|enabled| !enabled),
        "Could not confirm that all motors released; the arm may still be powered: {torque:?}"
    );
    Ok(())
}

fn report(message: &str) {
    // INVARIANT: The station may have died and closed the report pipe. Reporting must
    // never prevent the independent guard from taking over the arm.
    let _ = writeln!(
        std::io::stderr().lock(),
        "{}",
        serde_json::json!({ "guard": message })
    );
}

// WHY: The guard is reparented when its controller exits, so the parent ID detects that loss.
fn controller_alive(controller_pid: u32) -> bool {
    std::os::unix::process::parent_id() == controller_pid
}

fn kill_controller(controller_pid: u32) -> anyhow::Result<()> {
    if !controller_alive(controller_pid) {
        return Ok(());
    }
    let process_id = libc::pid_t::try_from(controller_pid)
        .context("The controller's process id is out of range")?;
    // SAFETY: kill only reads its plain arguments.
    if unsafe { libc::kill(process_id, libc::SIGKILL) } != 0 {
        let error = std::io::Error::last_os_error();
        // CONSTRAINT: A controller that exited in the meantime is what was wanted.
        if error.raw_os_error() != Some(libc::ESRCH) {
            return Err(error).context("Cannot kill the controller");
        }
    }
    let deadline = Instant::now() + CONTROLLER_EXIT;
    while controller_alive(controller_pid) {
        if Instant::now() > deadline {
            bail!("Controller did not exit after SIGKILL");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
pub enum GuardOutcome {
    ControllerDetached,
    TookOverMotorsAlreadyOff,
    HeldThenReleased { reason: String },
}

impl std::fmt::Display for GuardOutcome {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ControllerDetached => formatter
                .write_str("Controller released the arm and detached; the guard never took over"),
            Self::TookOverMotorsAlreadyOff => formatter.write_str(
                "Guard took over the bus, found the motors already off and released them",
            ),
            Self::HeldThenReleased { reason } => {
                write!(formatter, "Guard held the arm, then released it: {reason}")
            }
        }
    }
}

// WHY: Read the heartbeat pipe on its own thread, so the guard can time its silence.
fn listen_for_heartbeats(mut heartbeats: impl Read + Send + 'static) -> Receiver<Beat> {
    let (beats, listener) = channel();
    std::thread::spawn(move || {
        let mut buffer = [0_u8; HEARTBEAT_READ_BUFFER_BYTES];
        loop {
            match heartbeats.read(&mut buffer) {
                Ok(0) | Err(_) => {
                    let _ = beats.send(Beat::Closed);
                    return;
                }
                Ok(_) => {
                    if beats.send(Beat::Heartbeat).is_err() {
                        return;
                    }
                }
            }
        }
    });
    listener
}

enum Beat {
    Heartbeat,
    Closed,
}

// SAFETY: Open the exclusive bus only after controller heartbeats stop. Failure to take over or release may leave the arm powered and must be reported.
pub fn guard<A: GuardedArm>(
    config: &ControllerConfig,
    controller_pid: u32,
    heartbeats: impl Read + Send + 'static,
    mut open_arm: impl FnMut() -> anyhow::Result<A>,
) -> anyhow::Result<GuardOutcome> {
    let timing = config
        .guard_timing
        .context("The config has no guard section")?;
    println!("ready");
    std::io::stdout().flush()?;
    let listener = listen_for_heartbeats(heartbeats);

    let mut arm: Option<A> = None;
    let mut controller_attached = true;
    let mut hold: Option<HoldSupervisor> = None;
    let result = (|| -> anyhow::Result<GuardOutcome> {
        loop {
            let wait = if hold.is_some() || !controller_attached {
                timing.poll
            } else {
                timing.heartbeat_timeout
            };
            let heard = if controller_attached {
                listener.recv_timeout(wait)
            } else {
                std::thread::sleep(wait);
                Err(RecvTimeoutError::Timeout)
            };
            match heard {
                Ok(Beat::Heartbeat) => {
                    if hold.is_some() {
                        report("Controller heartbeat resumed");
                    }
                    hold = None;
                    continue;
                }
                Ok(Beat::Closed) | Err(RecvTimeoutError::Disconnected) => {
                    controller_attached = false;
                    if arm.is_none() && controller_alive(controller_pid) {
                        // INVARIANT: A live controller closes the pipe only after releasing
                        // the motors, and it still holds the bus.
                        return Ok(GuardOutcome::ControllerDetached);
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
            }
            if arm.is_none() {
                if controller_alive(controller_pid) {
                    report(&format!(
                        "Controller (pid {controller_pid}) stalled; stopping it to take over the bus"
                    ));
                    kill_controller(controller_pid).context(
                        "Could not stop the stalled controller; the arm is unsupervised",
                    )?;
                } else {
                    report(&format!(
                        "Controller (pid {controller_pid}) is gone; taking over the bus"
                    ));
                }
                arm = Some(open_arm().context(
                    "Could not open the bus to take over; the arm may still be powered and is unsupervised",
                )?);
            }
            let GuardReading { feedback, torque } = GuardReading::read(
                arm.as_mut().context(
                    "Guard lost its arm handle; the arm may still be powered and is unsupervised",
                )?,
                config,
            )?;
            if torque.iter().all(|enabled| !enabled) {
                if !controller_attached {
                    return Ok(GuardOutcome::TookOverMotorsAlreadyOff);
                }
                hold = None;
                continue;
            }
            let now = Instant::now();
            if hold.is_none() {
                hold = Some(HoldSupervisor::new(
                    config,
                    timing,
                    feedback.positions.clone(),
                    now,
                ));
                report(if controller_attached {
                    "Controller heartbeat lost; supervising hold"
                } else {
                    "Controller exited with motors enabled; supervising hold"
                });
            }
            if let Some(reason) = hold
                .as_ref()
                .and_then(|hold| hold.disable_reason(&feedback, &torque, now))
            {
                report(&format!("{reason}; disabling"));
                return Ok(GuardOutcome::HeldThenReleased { reason });
            }
        }
    })();
    if let Some(arm) = arm.as_mut() {
        release_and_confirm(arm).inspect_err(|error| {
            report(&format!(
                "Release failed; the arm may still be powered: {error:#}"
            ));
        })?;
        report("Arm released");
    }
    result
}

fn make_heartbeat_nonblocking(pipe: &impl AsRawFd) -> std::io::Result<()> {
    // SAFETY: fcntl borrows a live descriptor; neither operation transfers ownership.
    let flags = unsafe { libc::fcntl(pipe.as_raw_fd(), libc::F_GETFL) };
    if flags == -1
        || unsafe { libc::fcntl(pipe.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1
    {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

fn write_heartbeat(pipe: &mut impl Write) -> anyhow::Result<()> {
    // INVARIANT: A full pipe means the guard stopped consuming heartbeats. Report that
    // failure immediately rather than blocking the motor loop behind it.
    pipe.write_all(b".")
        .context("The safety guard heartbeat could not be delivered")
}

pub(crate) struct GuardProcess {
    child: Child,
    heartbeats: Option<ChildStdin>,
}

impl GuardProcess {
    pub fn start(config_path: &Path) -> anyhow::Result<Self> {
        let mut child = Command::new(crate::running_executable_for_child()?)
            .arg("guard")
            .arg("--config")
            .arg(config_path)
            .arg("--controller-pid")
            .arg(std::process::id().to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .context("Cannot start the guard")?;
        let heartbeats = child.stdin.take();
        if let Some(pipe) = &heartbeats {
            make_heartbeat_nonblocking(pipe)?;
        }
        let stdout = child.stdout.take().context("The guard has no stdout")?;
        let (ready, hearing) = channel();
        std::thread::spawn(move || {
            let mut line = String::new();
            let _ = std::io::BufRead::read_line(&mut std::io::BufReader::new(stdout), &mut line);
            let _ = ready.send(line);
        });
        match hearing.recv_timeout(SUPERVISOR_START) {
            Ok(line) if line.trim() == "ready" => Ok(Self { child, heartbeats }),
            Ok(_) => bail!("The safety guard exited before it was ready; check the logs"),
            Err(_) => bail!("The safety guard did not start within 10 seconds; try enabling again"),
        }
    }

    pub(crate) fn heartbeat(&mut self) -> anyhow::Result<()> {
        if let Some(status) = self
            .child
            .try_wait()
            .context("Cannot check whether the safety guard is alive")?
        {
            bail!("The safety guard exited with {status}");
        }
        let pipe = self.heartbeats.as_mut().context("The guard was detached")?;
        write_heartbeat(pipe)
    }

    #[must_use]
    pub(crate) fn detach(mut self) -> Vec<String> {
        let mut cleanup_errors = Vec::new();
        drop(self.heartbeats.take());
        let deadline = Instant::now() + DETACH_WAIT;
        loop {
            match self.child.try_wait() {
                Ok(Some(status)) if status.success() => break,
                Ok(Some(status)) => {
                    cleanup_errors.push(format!("The safety guard exited with {status}"));
                    break;
                }
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Ok(None) => {
                    cleanup_errors.push("The safety guard did not exit".to_owned());
                    break;
                }
                Err(error) => {
                    cleanup_errors.push(format!("The safety guard: {error}"));
                    break;
                }
            }
        }
        cleanup_errors
    }
}

// SAFETY: A plant with a guard process behind it: the guard starts before torque goes
// on, hears every cycle, and is let go once the motors are off.
pub struct GuardedPlant<P: Plant> {
    inner: P,
    config_path: Option<std::path::PathBuf>,
    guard: Option<GuardProcess>,
}

impl<P: Plant> GuardedPlant<P> {
    #[must_use]
    pub fn new(inner: P, config_path: Option<&Path>) -> Self {
        Self {
            inner,
            config_path: config_path.map(Path::to_owned),
            guard: None,
        }
    }

    #[must_use]
    pub fn inner(&self) -> &P {
        &self.inner
    }
}

impl<P: Plant> Plant for GuardedPlant<P> {
    fn read_feedback_into(&mut self, feedback: &mut Feedback) -> anyhow::Result<()> {
        self.inner.read_feedback_into(feedback)
    }

    fn command(&mut self, positions: &[f64], seconds: f64) -> anyhow::Result<()> {
        self.inner.command(positions, seconds)
    }

    fn command_gripper(&mut self, angle: f64) -> anyhow::Result<()> {
        self.inner.command_gripper(angle)
    }

    fn preflight(&mut self) -> anyhow::Result<()> {
        self.inner.preflight()
    }

    fn enable(&mut self) -> anyhow::Result<()> {
        self.inner.enable()
    }

    fn reconnect(&mut self) -> Reconnection {
        self.inner.reconnect()
    }

    fn observe_recovery(&mut self) -> anyhow::Result<crate::plant::RecoveryObservation> {
        self.inner.observe_recovery()
    }

    fn hold_recovery(&mut self) -> anyhow::Result<crate::plant::RecoveryObservation> {
        self.inner.hold_recovery()
    }

    fn enable_recovery(
        &mut self,
        drift_limit_radians: f64,
    ) -> anyhow::Result<crate::plant::RecoveryObservation> {
        self.inner.enable_recovery(drift_limit_radians)
    }

    fn release(&mut self) -> anyhow::Result<()> {
        // INVARIANT: A guard is let go only once the motors are off; after a failed
        // release it is kept, and with the heartbeats stopped it takes over
        // and supervises the hold.
        self.inner.release()?;
        let cleanup_errors = self.stop_guard();
        if let Some(error) = cleanup_errors.into_iter().next() {
            bail!(error);
        }
        Ok(())
    }

    fn start_guard(&mut self) -> anyhow::Result<()> {
        if self.guard.is_none()
            && let Some(config_path) = &self.config_path
        {
            let mut guard = GuardProcess::start(config_path)?;
            guard.heartbeat()?;
            self.guard = Some(guard);
        }
        Ok(())
    }

    fn heartbeat(&mut self) -> anyhow::Result<()> {
        match &mut self.guard {
            Some(guard) => guard.heartbeat(),
            None => Ok(()),
        }
    }

    fn stop_guard(&mut self) -> Vec<String> {
        self.guard
            .take()
            .map(GuardProcess::detach)
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    #[test]
    fn a_guard_that_stops_reading_cannot_block_the_heartbeat() {
        let (mut reader, mut writer) = std::io::pipe().unwrap();
        make_heartbeat_nonblocking(&writer).unwrap();
        write_heartbeat(&mut writer).unwrap();
        let mut byte = [0];
        reader.read_exact(&mut byte).unwrap();
        assert_eq!(byte, *b".");
        let filling = [b'.'; 4096];
        loop {
            match writer.write(&filling) {
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(error) => panic!("{error}"),
            }
        }
        let failure = write_heartbeat(&mut writer).unwrap_err();
        assert_eq!(
            failure.downcast_ref::<std::io::Error>().unwrap().kind(),
            std::io::ErrorKind::WouldBlock
        );
        reader.read_exact(&mut byte).unwrap();
        // CONSTRAINT: Drain enough to make room even on pipes that reserve space in pages.
        let mut drained = [0; 4095];
        reader.read_exact(&mut drained).unwrap();
        write_heartbeat(&mut writer).unwrap();
    }

    #[test]
    fn an_exited_guard_is_detected_even_if_its_heartbeat_pipe_stays_open() {
        let (reader, writer) = std::io::pipe().unwrap();
        let mut child = Command::new("true")
            .stdin(Stdio::from(reader.try_clone().unwrap()))
            .spawn()
            .unwrap();
        assert!(child.wait().unwrap().success());
        let heartbeat_descriptor: std::os::fd::OwnedFd = writer.into();
        let mut guard = GuardProcess {
            child,
            heartbeats: Some(ChildStdin::from(heartbeat_descriptor)),
        };
        let failure = guard.heartbeat().unwrap_err();
        assert!(failure.to_string().contains("safety guard exited"));
        drop(reader);
    }

    use crate::config::{GuardConfig, TEST_CONFIG};

    fn config() -> ControllerConfig {
        let mut config = ControllerConfig::parse(TEST_CONFIG).unwrap().into_input();
        config.guard = Some(GuardConfig {
            heartbeat_timeout_seconds: 0.05,
            poll_seconds: 0.01,
            hold_limit_seconds: 5.0,
        });
        ControllerConfig::try_from(config).unwrap()
    }

    fn feedback(positions: &[f64]) -> Feedback {
        Feedback {
            positions: positions.to_vec(),
            velocities: vec![0.0; positions.len()],
            efforts: vec![0.0; positions.len()],
            temperatures_celsius: Vec::new(),
            voltages_volts: Vec::new(),
            environment_confirmations: Vec::new(),
            gripper_angle: None,
            fault: None,
        }
    }

    #[test]
    fn a_hold_ends_on_a_fault_an_effort_spike_drift_lost_torque_or_the_time_limit() {
        let config = config();
        let timing = config.guard_timing.unwrap();
        let start = Instant::now();
        let n = config.joint_count;
        let rest = vec![0.0; n];
        let supervisor = HoldSupervisor::new(&config, timing, rest.clone(), start);
        let enabled = vec![true; n];
        assert_eq!(
            supervisor.disable_reason(&feedback(&rest), &enabled, start),
            None
        );

        let mut faulty = feedback(&rest);
        faulty.fault = Some("1: ok 80C".into());
        assert!(
            supervisor
                .disable_reason(&faulty, &enabled, start)
                .unwrap()
                .contains("80C")
        );

        let mut straining = feedback(&rest);
        straining.efforts[0] = config.joint_effort_limits[0] * 2.0;
        assert!(
            supervisor
                .disable_reason(&straining, &enabled, start)
                .unwrap()
                .contains("Effort")
        );

        let mut drifted = rest.clone();
        drifted[0] = config.tracking_static_margin_radians * 2.0;
        assert_eq!(
            supervisor.disable_reason(&feedback(&drifted), &enabled, start),
            Some("Arm moved during hold".into())
        );

        let mut lost = enabled.clone();
        lost[1] = false;
        assert!(
            supervisor
                .disable_reason(&feedback(&rest), &lost, start)
                .unwrap()
                .contains("torque")
        );

        let later = start + Duration::from_secs(6);
        assert!(
            supervisor
                .disable_reason(&feedback(&rest), &enabled, later)
                .unwrap()
                .contains("silent")
        );
    }

    struct ScriptedArm {
        readings: Vec<(Feedback, Vec<bool>)>,
        released: Arc<Mutex<bool>>,
    }

    impl GuardedArm for ScriptedArm {
        fn read(&mut self) -> anyhow::Result<(Feedback, Vec<bool>)> {
            let mut next = self.readings.remove(0);
            if self.readings.is_empty() {
                self.readings.push(next.clone());
            }
            if *self.released.lock().unwrap() {
                next.1.fill(false);
            }
            Ok(next)
        }

        fn release(&mut self) -> anyhow::Result<()> {
            *self.released.lock().unwrap() = true;
            Ok(())
        }
    }

    struct StuckPlant {
        release_works: bool,
    }

    impl Plant for StuckPlant {
        fn read_feedback_into(&mut self, reading: &mut Feedback) -> anyhow::Result<()> {
            *reading = feedback(&[0.0]);
            Ok(())
        }
        fn command(&mut self, _positions: &[f64], _seconds: f64) -> anyhow::Result<()> {
            Ok(())
        }
        fn command_gripper(&mut self, _angle: f64) -> anyhow::Result<()> {
            Ok(())
        }
        fn preflight(&mut self) -> anyhow::Result<()> {
            Ok(())
        }
        fn enable(&mut self) -> anyhow::Result<()> {
            Ok(())
        }
        fn release(&mut self) -> anyhow::Result<()> {
            anyhow::ensure!(self.release_works, "bus down");
            Ok(())
        }
    }

    fn plant_with_a_stand_in_guard(release_works: bool) -> GuardedPlant<StuckPlant> {
        let mut child = Command::new("cat")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        let heartbeats = child.stdin.take();
        if let Some(pipe) = &heartbeats {
            make_heartbeat_nonblocking(pipe).unwrap();
        }
        let mut plant = GuardedPlant::new(StuckPlant { release_works }, None);
        plant.guard = Some(GuardProcess { child, heartbeats });
        plant
    }

    #[test]
    fn a_failed_release_keeps_the_guard_and_a_successful_one_lets_it_go() {
        let mut stuck = plant_with_a_stand_in_guard(false);
        assert!(stuck.release().is_err());
        assert!(
            stuck.guard.is_some(),
            "the guard stays to supervise the hold"
        );
        if let Some(mut guard) = stuck.guard.take() {
            let _ = guard.child.kill();
        }
        let mut working = plant_with_a_stand_in_guard(true);
        working.release().unwrap();
        assert!(working.guard.is_none());
    }

    #[test]
    fn a_dead_controller_with_motors_on_leaves_the_arm_held_until_it_drifts_then_releases_it() {
        let config = config();
        let n = config.joint_count;
        let mut drifted = vec![0.0; n];
        drifted[0] = config.tracking_static_margin_radians * 2.0;
        let released = Arc::new(Mutex::new(false));
        let arm = Arc::new(Mutex::new(Some(ScriptedArm {
            readings: vec![
                (feedback(&vec![0.0; n]), vec![true; n]),
                (feedback(&vec![0.0; n]), vec![true; n]),
                (feedback(&drifted), vec![true; n]),
            ],
            released: Arc::clone(&released),
        })));
        // CONSTRAINT: The heartbeat pipe is already closed, and the controller (the real parent
        // of this test process is not it) counts as gone.
        let silent: &[u8] = &[];
        let outcome = guard(&config, u32::MAX, silent, || {
            Ok(arm.lock().unwrap().take().expect("opened once"))
        })
        .unwrap();
        assert!(*released.lock().unwrap(), "the guard released the arm");
        assert_eq!(
            outcome,
            GuardOutcome::HeldThenReleased {
                reason: "Arm moved during hold".into()
            }
        );
    }

    #[test]
    fn an_arm_already_without_torque_when_the_controller_goes_silent_is_released_not_held() {
        let config = config();
        let n = config.joint_count;
        let released = Arc::new(Mutex::new(false));
        let arm = Arc::new(Mutex::new(Some(ScriptedArm {
            readings: vec![(feedback(&vec![0.0; n]), vec![false; n])],
            released: Arc::clone(&released),
        })));
        let silent: &[u8] = &[];
        let outcome = guard(&config, u32::MAX, silent, || {
            Ok(arm.lock().unwrap().take().expect("opened once"))
        })
        .unwrap();
        assert!(*released.lock().unwrap());
        assert_eq!(outcome, GuardOutcome::TookOverMotorsAlreadyOff);
    }

    struct UnreleasableArm;

    impl GuardedArm for UnreleasableArm {
        fn read(&mut self) -> anyhow::Result<(Feedback, Vec<bool>)> {
            Ok((feedback(&[0.0, 0.0]), vec![false, false]))
        }
        fn release(&mut self) -> anyhow::Result<()> {
            bail!("bus down")
        }
    }

    #[test]
    fn a_release_that_fails_is_an_error_that_says_the_arm_may_still_be_powered() {
        let config = config();
        let silent: &[u8] = &[];
        let error = guard(&config, u32::MAX, silent, || Ok(UnreleasableArm)).unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("may still be powered"), "{message}");
        assert!(message.contains("bus down"), "{message}");
    }

    #[test]
    fn missing_torque_feedback_cannot_confirm_the_arm_was_released() {
        let config = config();
        let released = Arc::new(Mutex::new(false));
        let arm = Arc::new(Mutex::new(Some(ScriptedArm {
            readings: vec![(feedback(&[0.0]), Vec::new())],
            released: Arc::clone(&released),
        })));
        let error = guard(&config, u32::MAX, &[][..], || {
            Ok(arm.lock().unwrap().take().expect("opened once"))
        })
        .unwrap_err();
        assert!(*released.lock().unwrap());
        assert!(format!("{error:#}").contains("may still be powered"));
    }

    #[test]
    fn a_release_command_that_leaves_torque_on_is_not_reported_as_success() {
        struct TorqueRemainsEnabled;

        impl GuardedArm for TorqueRemainsEnabled {
            fn read(&mut self) -> anyhow::Result<(Feedback, Vec<bool>)> {
                Ok((feedback(&[0.0]), vec![true]))
            }

            fn release(&mut self) -> anyhow::Result<()> {
                Ok(())
            }
        }

        let error = release_and_confirm(&mut TorqueRemainsEnabled).unwrap_err();
        assert!(format!("{error:#}").contains("may still be powered"));
    }

    #[test]
    fn a_bus_that_cannot_be_opened_during_takeover_is_an_error_naming_the_unsupervised_arm() {
        let config = config();
        let silent: &[u8] = &[];
        let error =
            guard::<UnreleasableArm>(&config, u32::MAX, silent, || bail!("port busy")).unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("unsupervised"), "{message}");
        assert!(message.contains("port busy"), "{message}");
    }

    #[test]
    fn malformed_guard_feedback_releases_instead_of_establishing_a_hold() {
        for positions in [vec![f64::NAN, 0.0], vec![0.0]] {
            let config = config();
            let released = Arc::new(Mutex::new(false));
            let mut arm = Some(ScriptedArm {
                readings: vec![(feedback(&positions), vec![true; config.joint_count])],
                released: Arc::clone(&released),
            });
            let error = guard(&config, u32::MAX, &[] as &[u8], || {
                arm.take()
                    .ok_or_else(|| anyhow::anyhow!("Arm already opened"))
            })
            .unwrap_err();
            assert!(format!("{error:#}").contains("malformed feedback"));
            assert!(*released.lock().unwrap());
        }
    }
}

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    reason = "This integration-test target may abort on failed setup or unexpected outcomes."
)]
#![allow(dead_code, reason = "each test crate uses part of this")]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use hebbian_contracts::{Command, CommandRecord, Event, Frame};
use hebbian_robot_use_core::control_link::LinkHandle;
use hebbian_robot_use_core::control_loop::{ArmSetup, run_arms};
use hebbian_robot_use_core::controller_transport::{ControllerAnswer, ControllerRequest};
use hebbian_robot_use_core::plant::Plant;
use tokio::sync::mpsc::Receiver;
use tokio::sync::mpsc::error::TryRecvError;

const QUEUE_CAPACITY: usize = 10_000;
const WAIT: Duration = Duration::from_secs(10);

pub struct LinkClient {
    handle: LinkHandle,
    client: u64,
    frames: Receiver<Frame>,
}

impl LinkClient {
    pub fn connect(handle: &LinkHandle) -> Self {
        let (client, frames) = handle.connect(QUEUE_CAPACITY);
        Self {
            handle: handle.clone(),
            client,
            frames,
        }
    }

    pub fn submit(&self, request: ControllerRequest) -> ControllerAnswer {
        match self.handle.controller_loop_arm().submit(request) {
            Ok(pending) => pending.wait(),
            Err(refusal) => refusal,
        }
    }

    pub fn command(&self, command: Command) -> CommandRecord {
        match self.submit(ControllerRequest::Submit {
            command,
            trace_context: None,
        }) {
            ControllerAnswer::Submitted(record) => *record,
            answer => panic!("command was not accepted: {answer:?}"),
        }
    }

    pub fn snapshot(&self) {
        self.handle.snapshot(self.client);
    }

    pub fn next_frame(&mut self) -> Frame {
        let deadline = Instant::now() + WAIT;
        loop {
            match self.frames.try_recv() {
                Ok(frame) => return frame,
                Err(TryRecvError::Empty) => {
                    assert!(Instant::now() < deadline, "no frame in {WAIT:?}");
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(TryRecvError::Disconnected) => panic!("the executor dropped the client"),
            }
        }
    }

    pub fn drain_events(&mut self) -> Vec<Event> {
        let mut events = Vec::new();
        while let Ok(frame) = self.frames.try_recv() {
            events.push(frame.event);
        }
        events
    }

    pub fn read_until(&mut self, mut done: impl FnMut(&Event) -> bool) -> Event {
        let deadline = Instant::now() + WAIT;
        loop {
            assert!(Instant::now() < deadline, "no matching event in {WAIT:?}");
            let event = self.next_frame().event;
            if done(&event) {
                return event;
            }
        }
    }
}

// SAFETY: Stop simulated arms as soon as the test finishes, including after a panic.
pub struct RunningArms {
    shutdown_requested: Arc<AtomicBool>,
    loop_thread: Option<JoinHandle<()>>,
}

impl RunningArms {
    pub fn start<ArmPlant: Plant + Send + 'static>(mut arms: Vec<ArmSetup<ArmPlant>>) -> Self {
        let shutdown_requested = Arc::new(AtomicBool::new(false));
        let loop_shutdown_requested = Arc::clone(&shutdown_requested);
        let loop_thread = std::thread::spawn(move || {
            run_arms(
                &mut arms,
                Some(Duration::from_secs(30)),
                &loop_shutdown_requested,
            );
            for arm in arms {
                if let Some(link) = arm.link {
                    link.close();
                }
            }
        });
        Self {
            shutdown_requested,
            loop_thread: Some(loop_thread),
        }
    }
}

impl Drop for RunningArms {
    fn drop(&mut self) {
        self.shutdown_requested.store(true, Ordering::Relaxed);
        if let Some(loop_thread) = self.loop_thread.take() {
            let result = loop_thread.join();
            if !std::thread::panicking() {
                result.expect("the simulated control loop did not panic");
            }
        }
    }
}

#![allow(dead_code, reason = "each test crate uses part of this")]
//! A client of an arm's in-process link: requests go in as lines and the arm's
//! frames come back on a queue.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use hebbian_contracts::{Event, Frame};
use hebbian_robot_use_server::control_link::LinkHandle;
use hebbian_robot_use_server::control_loop::{ArmSetup, run_arms};
use hebbian_robot_use_server::plant::Plant;
use serde_json::{Value, json};
use tokio::sync::mpsc::Receiver;
use tokio::sync::mpsc::error::TryRecvError;

/// Lines a client may leave unread before it counts as not reading.
const QUEUE_CAPACITY: usize = 10_000;
const WAIT: Duration = Duration::from_secs(10);

pub struct LinkClient {
    handle: LinkHandle,
    client: u64,
    lines: Receiver<String>,
}

impl LinkClient {
    pub fn connect(handle: &LinkHandle) -> Self {
        let (client, lines) = handle.connect(QUEUE_CAPACITY);
        Self {
            handle: handle.clone(),
            client,
            lines,
        }
    }

    /// Send one raw line. The arm answers it at the start of its next cycle, so
    /// its loop must be running.
    pub fn send_line(&self, line: &str) {
        self.handle.request(self.client, line);
    }

    /// Send a request, with a request id when given.
    pub fn send_request(&self, request_id: Option<&str>, request: &Value) {
        let mut envelope = json!({"request": request});
        if let Some(id) = request_id {
            envelope["request_id"] = json!(id);
        }
        self.send_line(&envelope.to_string());
    }

    /// Send a command, with a request id when given.
    pub fn send_command(&self, request_id: Option<&str>, command: &Value) {
        self.send_request(request_id, &json!({"kind": "command", "command": command}));
    }

    pub fn next_frame(&mut self) -> Frame {
        let deadline = Instant::now() + WAIT;
        loop {
            match self.lines.try_recv() {
                Ok(line) => return serde_json::from_str(&line).unwrap(),
                Err(TryRecvError::Empty) => {
                    assert!(Instant::now() < deadline, "no frame in {WAIT:?}");
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(TryRecvError::Disconnected) => panic!("the executor dropped the client"),
            }
        }
    }

    /// Everything the arm sent that has not been read, once its link is closed.
    pub fn drain_events(&mut self) -> Vec<Event> {
        let mut events = Vec::new();
        while let Ok(line) = self.lines.try_recv() {
            events.push(serde_json::from_str::<Frame>(&line).unwrap().event);
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

/// Stop simulated arms as soon as the test finishes, including after a panic.
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

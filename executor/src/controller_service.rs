//! The controller-side transport worker. None of its I/O runs on the motor loop.

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError, sync_channel};
use std::thread;
use std::time::{Duration, Instant};

use hebbian_contracts::{Event, Frame, RejectionReason};

use crate::control_link::Link;
use crate::control_link::LinkHandle;
use crate::controller_bridge::StationSession;
use crate::controller_transport::{
    ControllerAnswer, ControllerInstanceId, ControllerMessage, ControllerRequestId,
    ControllerTransport,
};

const OUTBOUND_CAPACITY: usize = 512;
const MAXIMUM_IN_FLIGHT_REQUESTS: usize = 128;
const WRITE_TIMEOUT: Duration = Duration::from_secs(2);
const STATION_POLL: Duration = Duration::from_millis(5);
const INITIAL_FEEDBACK_TIMEOUT: Duration = Duration::from_secs(10);
const UNGUARDED_FEEDBACK_TIMEOUT: Duration = Duration::from_secs(3);
const CONTROLLER_STARTUP_TIMEOUT: Duration = Duration::from_secs(60);

fn close_connection(stream: &UnixStream, stopped: &AtomicBool) {
    stopped.store(true, Ordering::Release);
    let _ = stream.shutdown(Shutdown::Both);
}

fn check_feedback_deadlines(
    feedback_times: &BTreeMap<String, Option<Instant>>,
    links: &BTreeMap<String, Arc<Link>>,
    started_at: Instant,
) -> io::Result<()> {
    for (arm, last_feedback) in feedback_times {
        let timeout = last_feedback.map_or(INITIAL_FEEDBACK_TIMEOUT, |_| {
            links[arm]
                .handle()
                .config()
                .guard
                .as_ref()
                .map_or(UNGUARDED_FEEDBACK_TIMEOUT, |guard| {
                    Duration::from_secs_f64(guard.heartbeat_timeout_seconds)
                })
        });
        if last_feedback.unwrap_or(started_at).elapsed() > timeout {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!(
                    "No fresh feedback from {arm} for {:.1} seconds",
                    timeout.as_secs_f64()
                ),
            ));
        }
    }
    Ok(())
}

fn wait_for_controller_ready(
    messages: &std::sync::mpsc::Receiver<io::Result<ControllerMessage>>,
    shutdown: &AtomicBool,
) -> io::Result<()> {
    let startup_deadline = Instant::now() + CONTROLLER_STARTUP_TIMEOUT;
    loop {
        if shutdown.load(Ordering::Acquire) || Instant::now() >= startup_deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Controller did not report fresh disabled feedback",
            ));
        }
        match messages.recv_timeout(STATION_POLL) {
            Ok(Ok(ControllerMessage::Ready)) => return Ok(()),
            Ok(Ok(_)) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Controller did not report readiness first",
                ));
            }
            Ok(Err(error)) => return Err(error),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                return Err(io::Error::other("Controller reader stopped"));
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
}

fn close_subscriptions(
    socket: &UnixStream,
    stopped: &AtomicBool,
    subscriptions: Vec<(LinkHandle, u64)>,
) {
    close_connection(socket, stopped);
    for (link, client) in subscriptions {
        link.disconnect(client);
    }
}

/// Bridge a verified controller to the station until it disconnects.
/// `on_disconnect` invalidates the station lease after the handoff is fenced.
/// The controller must have exclusive bus ownership before connecting. The
/// readiness message confirms its startup checks before new commands are admitted.
///
/// # Errors
/// The connection fails, the peer violates the protocol, or activation fails.
pub fn serve_station_connection(
    socket: UnixStream,
    instance: ControllerInstanceId,
    session: &mut StationSession,
    links: &BTreeMap<String, Arc<Link>>,
    shutdown: &AtomicBool,
    on_ready: impl FnOnce(),
    on_disconnect: impl FnOnce(),
) -> io::Result<()> {
    socket.set_write_timeout(Some(WRITE_TIMEOUT))?;
    let mut reader = ControllerTransport::new(socket.try_clone()?, instance);
    let writer_socket = socket.try_clone()?;
    let (incoming, messages) = sync_channel(OUTBOUND_CAPACITY);
    thread::spawn(move || {
        loop {
            let received = reader.receive();
            let failed = received.is_err();
            if incoming.send(received).is_err() || failed {
                break;
            }
        }
    });
    if let Err(error) = wait_for_controller_ready(&messages, shutdown)
        .and_then(|()| session.activate(instance).map_err(io::Error::other))
    {
        let _ = socket.shutdown(Shutdown::Both);
        return Err(error);
    }
    on_ready();
    let mut writer = ControllerTransport::new(writer_socket, instance);
    let started_at = Instant::now();
    let mut feedback_times: BTreeMap<String, Option<Instant>> =
        links.keys().map(|name| (name.clone(), None)).collect();
    let result = (|| -> io::Result<()> {
        while !shutdown.load(Ordering::Acquire) {
            check_feedback_deadlines(&feedback_times, links, started_at)?;
            if let Some(message) = session.next_message() {
                writer.send(message)?;
            }
            match messages.recv_timeout(STATION_POLL) {
                Ok(Ok(ControllerMessage::Answer { request_id, answer })) => {
                    if !session.answer(instance, request_id, answer, |arm, record| {
                        if let Some(link) = links.get(arm) {
                            link.ensure_controller_record(record.clone());
                        }
                    }) {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "Controller answered an unknown request",
                        ));
                    }
                }
                Ok(Ok(ControllerMessage::Event { arm, event })) => {
                    let link = links.get(&arm).ok_or_else(|| {
                        io::Error::new(
                            io::ErrorKind::InvalidData,
                            "Controller reported an unknown arm",
                        )
                    })?;
                    if !matches!(*event, Event::State(_)) && !event.is_logged() {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "Controller sent an invalid event",
                        ));
                    }
                    if matches!(*event, Event::State(_)) {
                        feedback_times.insert(arm, Some(Instant::now()));
                    }
                    link.publish(*event);
                }
                Ok(Ok(ControllerMessage::Request { .. })) => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Controller sent a request to the station",
                    ));
                }
                Ok(Ok(ControllerMessage::Ready)) => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Controller repeated its readiness message",
                    ));
                }
                Ok(Err(error)) => return Err(error),
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(io::Error::other("Controller reader stopped"));
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
        Ok(())
    })();
    session.disconnect();
    on_disconnect();
    let _ = socket.shutdown(Shutdown::Both);
    result
}

fn forward_events(
    arm: String,
    link: &LinkHandle,
    messages: SyncSender<ControllerMessage>,
    socket: UnixStream,
    stopped: Arc<AtomicBool>,
) -> u64 {
    let (client, mut events) = link.connect(OUTBOUND_CAPACITY);
    thread::spawn(move || {
        while let Some(line) = events.blocking_recv() {
            let Ok(frame) = serde_json::from_str::<Frame>(&line) else {
                close_connection(&socket, &stopped);
                return;
            };
            if !matches!(frame.event, Event::State(_)) && !frame.event.is_logged() {
                continue;
            }
            let message = ControllerMessage::Event {
                arm: arm.clone(),
                event: Box::new(frame.event),
            };
            match messages.try_send(message) {
                Ok(()) => {}
                // Samples may be skipped, but losing a logged outcome would make
                // the station's command history unreliable.
                Err(TrySendError::Full(ControllerMessage::Event { event, .. }))
                    if matches!(*event, Event::State(_)) => {}
                Err(_) => {
                    close_connection(&socket, &stopped);
                    return;
                }
            }
        }
        if !stopped.load(Ordering::Acquire) {
            close_connection(&socket, &stopped);
        }
    });
    client
}

fn write_controller_messages(
    mut transport: ControllerTransport<UnixStream>,
    messages: std::sync::mpsc::Receiver<ControllerMessage>,
    arms: &BTreeMap<String, LinkHandle>,
    ready_to_accept: &AtomicBool,
) -> io::Result<()> {
    let mut awaiting: BTreeSet<String> = arms.keys().cloned().collect();
    let mut first_samples = BTreeMap::new();
    let mut startup_events = Vec::new();
    let mut ready = false;
    while let Ok(message) = messages.recv() {
        if ready {
            transport.send(message)?;
            continue;
        }
        match message {
            ControllerMessage::Event { arm, event } => match *event {
                Event::State(sample) => {
                    if sample.mode != hebbian_contracts::ArmMode::Disabled
                        || sample.fault.is_some()
                        || sample.positions.is_empty()
                        || sample
                            .positions
                            .iter()
                            .any(|position| !position.is_finite())
                    {
                        return Err(io::Error::other(format!(
                            "Arm {arm} did not start disabled with healthy feedback"
                        )));
                    }
                    awaiting.remove(&arm);
                    first_samples.insert(
                        arm.clone(),
                        (
                            Instant::now(),
                            ControllerMessage::Event {
                                arm,
                                event: Box::new(Event::State(sample)),
                            },
                        ),
                    );
                }
                event => {
                    if startup_events.len() >= OUTBOUND_CAPACITY {
                        return Err(io::Error::other("Too many controller startup events"));
                    }
                    startup_events.push(ControllerMessage::Event {
                        arm,
                        event: Box::new(event),
                    });
                }
            },
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Unexpected message before controller readiness",
                ));
            }
        }
        if awaiting.is_empty() {
            for (arm, (sampled_at, _)) in &first_samples {
                let timeout = arms[arm]
                    .config()
                    .guard
                    .as_ref()
                    .map_or(UNGUARDED_FEEDBACK_TIMEOUT, |guard| {
                        Duration::from_secs_f64(guard.heartbeat_timeout_seconds)
                    });
                if sampled_at.elapsed() > timeout {
                    awaiting.insert(arm.clone());
                }
            }
        }
        if awaiting.is_empty() {
            ready_to_accept.store(true, Ordering::Release);
            transport.send(ControllerMessage::Ready)?;
            for event in startup_events.drain(..) {
                transport.send(event)?;
            }
            for (_, (_, sample)) in std::mem::take(&mut first_samples) {
                transport.send(sample)?;
            }
            ready = true;
        }
    }
    Ok(())
}

/// Serve one controller instance until its station disconnects or the protocol
/// fails. The caller must stop the control loop when this returns.
///
/// # Errors
/// The socket fails, a message is invalid, or a request ID is repeated.
pub fn serve_controller(
    socket: UnixStream,
    instance: ControllerInstanceId,
    arms: &BTreeMap<String, LinkHandle>,
) -> io::Result<()> {
    socket.set_write_timeout(Some(WRITE_TIMEOUT))?;
    let stopped = Arc::new(AtomicBool::new(false));
    let inflight = Arc::new(AtomicUsize::new(0));
    let (outbound, messages) = sync_channel::<ControllerMessage>(OUTBOUND_CAPACITY);
    let writer_socket = socket.try_clone()?;
    let writer_failure_socket = socket.try_clone()?;
    let writer_stop = Arc::clone(&stopped);
    let writer_arms = arms.clone();
    let ready_to_accept = Arc::new(AtomicBool::new(false));
    let writer_ready = Arc::clone(&ready_to_accept);
    let writer = thread::spawn(move || {
        let transport = ControllerTransport::new(writer_socket, instance);
        if let Err(error) =
            write_controller_messages(transport, messages, &writer_arms, &writer_ready)
        {
            eprintln!("Controller output failed: {error}");
            close_connection(&writer_failure_socket, &writer_stop);
        }
    });
    let mut subscriptions = Vec::new();
    for (arm, link) in arms {
        let client = forward_events(
            arm.clone(),
            link,
            outbound.clone(),
            socket.try_clone()?,
            Arc::clone(&stopped),
        );
        subscriptions.push((link.clone(), client));
    }
    let mut reader = ControllerTransport::new(socket.try_clone()?, instance);
    let mut last_request: Option<ControllerRequestId> = None;
    let result = loop {
        if stopped.load(Ordering::Acquire) {
            break Err(io::Error::other("Controller event transport stopped"));
        }
        let message = match reader.receive() {
            Ok(message) => message,
            Err(error) => break Err(error),
        };
        let ControllerMessage::Request {
            request_id,
            arm,
            request,
        } = message
        else {
            break Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Expected a controller request",
            ));
        };
        if !ready_to_accept.load(Ordering::Acquire) {
            break Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Controller received a command before it was ready",
            ));
        }
        if last_request.is_some_and(|last| request_id.0 <= last.0) {
            break Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Controller request ID was repeated or out of order",
            ));
        }
        last_request = Some(request_id);
        if inflight.fetch_add(1, Ordering::AcqRel) >= MAXIMUM_IN_FLIGHT_REQUESTS {
            inflight.fetch_sub(1, Ordering::AcqRel);
            break Err(io::Error::other("Too many controller requests in flight"));
        }
        let answer = arms.get(&arm).map_or_else(
            || {
                Err(ControllerAnswer::Refused {
                    reason: RejectionReason::InvalidTarget,
                    detail: format!("No arm {arm}"),
                })
            },
            |link| link.controller_loop_arm().submit(request),
        );
        let outbound = outbound.clone();
        let inflight = Arc::clone(&inflight);
        let socket = socket.try_clone()?;
        let stopped = Arc::clone(&stopped);
        thread::spawn(move || {
            let answer = answer.map_or_else(
                |refused| refused,
                crate::controller_bridge::PendingLoopAnswer::wait,
            );
            if outbound
                .send(ControllerMessage::Answer { request_id, answer })
                .is_err()
            {
                close_connection(&socket, &stopped);
            }
            inflight.fetch_sub(1, Ordering::AcqRel);
        });
    };
    close_subscriptions(&socket, &stopped, subscriptions);
    drop(outbound);
    // Pending loop requests can remain blocked while the loop is stalled. The
    // process supervisor is responsible for ending this process in that case.
    drop(writer);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arm::ArmCore;
    use crate::config::{ExecutorConfig, TEST_CONFIG};
    use crate::control_link::Link;
    use crate::controller_transport::ControllerRequest;
    use crate::handoff::apply_request;
    use crate::lease::{LEASE, Lease};
    use hebbian_contracts::Command;
    use std::time::Instant;

    fn publish_disabled_feedback(
        link: Arc<Link>,
        stopped: Arc<AtomicBool>,
    ) -> thread::JoinHandle<()> {
        thread::spawn(move || {
            while !stopped.load(Ordering::Acquire) {
                let mut sample = hebbian_contracts::Sample::blank();
                sample.positions = vec![0.0, 0.0];
                link.publish(Event::State(sample));
                thread::sleep(Duration::from_millis(10));
            }
        })
    }

    #[test]
    fn readiness_waits_for_fresh_disabled_loop_feedback() {
        let config = ExecutorConfig::parse(TEST_CONFIG).unwrap();
        let link = Arc::new(Link::new(&config, Arc::new(Lease::new(LEASE))).unwrap());
        let arms = BTreeMap::from([("arm".to_owned(), link.handle())]);
        let (station_socket, controller_socket) = UnixStream::pair().unwrap();
        station_socket
            .set_read_timeout(Some(Duration::from_millis(150)))
            .unwrap();
        let instance = ControllerInstanceId([3; 16]);
        let service = thread::spawn(move || serve_controller(controller_socket, instance, &arms));
        let mut station = ControllerTransport::new(station_socket, instance);
        assert!(
            station.receive().is_err(),
            "preflight alone must not report readiness"
        );
        let publishing = Arc::new(AtomicBool::new(false));
        let publisher = publish_disabled_feedback(Arc::clone(&link), Arc::clone(&publishing));
        assert!(matches!(
            station.receive().unwrap(),
            ControllerMessage::Ready
        ));
        publishing.store(true, Ordering::Release);
        publisher.join().unwrap();
        drop(station);
        assert!(service.join().unwrap().is_err());
        Arc::try_unwrap(link).ok().unwrap().close();
    }

    #[test]
    fn a_controller_executes_a_request_once_and_rejects_a_repeated_id() {
        let config = ExecutorConfig::parse(TEST_CONFIG).unwrap();
        let link = Arc::new(Link::new(&config, Arc::new(Lease::new(LEASE))).unwrap());
        let inbox = link.take_loop_end().unwrap().inbox;
        let arms = BTreeMap::from([("arm".to_owned(), link.handle())]);
        let (station_socket, controller_socket) = UnixStream::pair().unwrap();
        station_socket
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let instance = ControllerInstanceId([7; 16]);
        let service = thread::spawn(move || serve_controller(controller_socket, instance, &arms));
        let publishing = Arc::new(AtomicBool::new(false));
        let publisher = publish_disabled_feedback(Arc::clone(&link), Arc::clone(&publishing));
        let mut station = ControllerTransport::new(station_socket, instance);
        assert!(matches!(
            station.receive().unwrap(),
            ControllerMessage::Ready
        ));
        publishing.store(true, Ordering::Release);
        publisher.join().unwrap();
        let request = || ControllerMessage::Request {
            request_id: ControllerRequestId(0),
            arm: "arm".into(),
            request: ControllerRequest::Submit {
                command: Command::Enable,
                trace_context: None,
            },
        };
        station.send(request()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        let admitted = loop {
            if let Some(request) = inbox.next() {
                break request;
            }
            assert!(Instant::now() < deadline, "the request reached the loop");
            thread::sleep(Duration::from_millis(1));
        };
        let mut core = ArmCore::new(config.arm_limits(), |_| {});
        apply_request(admitted, "arm", &mut BTreeMap::from([("arm", &mut core)]));
        loop {
            if matches!(
                station.receive().unwrap(),
                ControllerMessage::Answer {
                    request_id: ControllerRequestId(0),
                    answer: ControllerAnswer::Submitted(_)
                }
            ) {
                break;
            }
        }
        station.send(request()).unwrap();
        assert!(service.join().unwrap().is_err());
        assert!(
            inbox.next().is_none(),
            "the repeated command was not enqueued"
        );
        Arc::try_unwrap(link).ok().unwrap().close();
    }

    #[test]
    fn station_and_controller_exchange_a_command_and_fence_requests_on_disconnect() {
        let config = ExecutorConfig::parse(TEST_CONFIG).unwrap();
        let station_link = Arc::new(Link::new(&config, Arc::new(Lease::new(LEASE))).unwrap());
        let controller_link = Arc::new(Link::new(&config, Arc::new(Lease::new(LEASE))).unwrap());
        let controller_inbox = controller_link.take_loop_end().unwrap().inbox;
        let station_arm = station_link.take_controller_station_arm().unwrap();
        let mut session = StationSession::new(BTreeMap::from([("arm".into(), station_arm)]));
        let station_links = BTreeMap::from([("arm".into(), Arc::clone(&station_link))]);
        let controller_links = BTreeMap::from([("arm".into(), controller_link.handle())]);
        let (station_socket, controller_socket) = UnixStream::pair().unwrap();
        let disconnect_socket = station_socket.try_clone().unwrap();
        let instance = ControllerInstanceId([9; 16]);
        let disconnected = Arc::new(AtomicBool::new(false));
        let disconnect_observed = Arc::clone(&disconnected);
        let station = thread::spawn(move || {
            serve_station_connection(
                station_socket,
                instance,
                &mut session,
                &station_links,
                &AtomicBool::new(false),
                || {},
                || disconnect_observed.store(true, Ordering::Release),
            )
        });
        let controller =
            thread::spawn(move || serve_controller(controller_socket, instance, &controller_links));
        let publishing = Arc::new(AtomicBool::new(false));
        let publisher =
            publish_disabled_feedback(Arc::clone(&controller_link), Arc::clone(&publishing));
        let deadline = Instant::now() + Duration::from_secs(3);
        let pending = loop {
            if let Ok(pending) =
                station_link
                    .controller_loop_arm()
                    .submit(ControllerRequest::Submit {
                        command: Command::Enable,
                        trace_context: None,
                    })
            {
                break pending;
            }
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(1));
        };
        let admitted = loop {
            if let Some(request) = controller_inbox.next() {
                break request;
            }
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(1));
        };
        publishing.store(true, Ordering::Release);
        publisher.join().unwrap();
        let mut core = ArmCore::new(config.arm_limits(), |_| {});
        apply_request(admitted, "arm", &mut BTreeMap::from([("arm", &mut core)]));
        let ControllerAnswer::Submitted(record) = pending.wait() else {
            panic!("command was not accepted")
        };
        let command_id = record.command_id.clone();
        disconnect_socket.shutdown(Shutdown::Both).unwrap();
        assert!(station.join().unwrap().is_err());
        assert!(controller.join().unwrap().is_err());
        station_link.controller_unavailable("Controller exited".into());
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let recorded = runtime
            .block_on(station_link.handle().find_record(command_id))
            .expect("accepted record survived controller loss");
        assert_eq!(
            recorded.outcome(),
            Some(hebbian_contracts::CommandOutcome::Interrupted)
        );
        assert!(disconnected.load(Ordering::Acquire));
        assert!(
            station_link
                .controller_loop_arm()
                .submit(ControllerRequest::Submit {
                    command: Command::Enable,
                    trace_context: None
                })
                .is_err()
        );
        Arc::try_unwrap(controller_link).ok().unwrap().close();
        Arc::try_unwrap(station_link).ok().unwrap().close();
    }
}

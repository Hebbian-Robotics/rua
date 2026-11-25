//! The resumable HTTP event stream and its cached station observations.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::convert::Infallible;
use std::sync::Mutex;
use std::time::Duration;

use hebbian_contracts::{
    ActionSequenceRecord, ArmRecord, Event, Frame, LiveRun, MAX_RUN_REPLAY_RECORDS, RunEvent,
    ServerMessage, StationEventSnapshot, StationState, TraceSpan, UnixNanos,
};
use tokio::sync::{broadcast, watch};

enum StreamUpdate {
    Continue,
    SnapshotRequired,
}

const RECONNECT_DELAY_CAP: Duration = Duration::from_secs(1);
const EVENT_READ_TIMEOUT: Duration = Duration::from_secs(30);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
const RETAINED_RECORDS: usize = 10_000;

/// What the telemetry stream of a model loop's run reports.
#[derive(Debug, Clone)]
pub enum RunUpdate {
    Event {
        seq: u64,
        received_unix_nano: UnixNanos,
        event: RunEvent,
    },
    Span {
        seq: u64,
        received_unix_nano: UnixNanos,
        span: Box<TraceSpan>,
    },
    /// Records after the cursor fell out of the retained buffer; the stream
    /// continues after `newest`.
    Resync {
        oldest_retained: u64,
        newest: u64,
    },
    Snapshot(Box<LiveRun>),
}

#[derive(serde::Deserialize)]
#[serde(untagged)]
enum Incoming {
    Control(StreamControl),
    Known(Box<ServerMessage>),
    /// A message this SDK could not decode, including newer variants nested in arm frames.
    Unsupported {
        kind: String,
    },
}

#[derive(serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum StreamControl {
    ResyncRequired,
}

#[derive(Default)]
enum LeaseState {
    #[default]
    Idle,
    Acquiring,
    Held {
        token: String,
    },
}

impl LeaseState {
    fn token(&self) -> Option<&str> {
        match self {
            Self::Held { token } => Some(token),
            Self::Idle | Self::Acquiring => None,
        }
    }

    fn clear_if_held_by(&mut self, token: &str) -> bool {
        if self.token() == Some(token) {
            *self = Self::Idle;
            true
        } else {
            false
        }
    }

    fn lose(&mut self) -> bool {
        if matches!(self, Self::Held { .. }) {
            *self = Self::Idle;
            true
        } else {
            false
        }
    }
}

#[derive(Default)]
enum EventStreamState {
    #[default]
    Starting,
    Streaming,
    Disconnected {
        since: tokio::time::Instant,
        error: String,
    },
}

impl EventStreamState {
    fn disconnected_since(&self) -> Option<tokio::time::Instant> {
        match self {
            Self::Disconnected { since, .. } => Some(*since),
            Self::Starting | Self::Streaming => None,
        }
    }
}

#[derive(Default)]
pub(crate) struct Inner {
    stream: EventStreamState,
    connection_generation: u64,
    event_cursor: Option<String>,
    pub(crate) state: Option<StationState>,
    /// State frames received so far, so a waiter can tell a frame that arrived after
    /// some event from the cached one.
    pub(crate) state_count: u64,
    pub(crate) action_sequence: Option<ActionSequenceRecord>,
    pub(crate) action_sequence_revision: u64,
    records: HashMap<hebbian_contracts::CommandId, ArmRecord>,
    record_order: VecDeque<hebbian_contracts::CommandId>,
    lease_state: LeaseState,
    lease_loss_status: bool,
    resume_after: BTreeMap<String, u64>,
    run_after: Option<u64>,
}

impl Inner {
    pub(crate) fn is_connected(&self) -> bool {
        matches!(self.stream, EventStreamState::Streaming)
    }

    pub(crate) fn handshake_result(&self) -> Option<Result<(), String>> {
        match &self.stream {
            EventStreamState::Streaming => Some(Ok(())),
            EventStreamState::Disconnected { error, .. } if self.connection_generation == 0 => {
                Some(Err(error.clone()))
            }
            EventStreamState::Starting | EventStreamState::Disconnected { .. } => None,
        }
    }

    fn observation_cursor(&self) -> (u64, u64) {
        (self.state_count, self.connection_generation)
    }

    pub(crate) fn record(&self, command_id: &str) -> Option<&ArmRecord> {
        self.records.get(command_id)
    }

    pub(crate) fn keep_record(&mut self, arm_record: ArmRecord) {
        let record = &arm_record.record;
        // INVARIANT: A late, unfinished revision never replaces the finished one.
        if let Some(previous) = self.records.get(&record.command_id)
            && !previous.record.is_pending()
            && record.is_pending()
        {
            return;
        }
        let id = record.command_id.clone();
        if self.records.insert(id.clone(), arm_record).is_none() {
            self.record_order.push_back(id);
            if self.record_order.len() > RETAINED_RECORDS
                && let Some(oldest) = self.record_order.pop_front()
            {
                self.records.remove(&oldest);
            }
        }
    }

    fn keep_frame(&mut self, arm: &str, frame: &Frame) {
        if frame.seq.is_some_and(|action_sequence| {
            self.resume_after
                .get(arm)
                .is_some_and(|previous| action_sequence <= *previous)
        }) {
            return;
        }
        let cursor = match &frame.event {
            Event::Record(record) => {
                self.keep_record(ArmRecord {
                    arm: arm.to_owned(),
                    record: record.clone(),
                });
                frame.seq
            }
            Event::Snapshot {
                seq, records, log, ..
            } => {
                for retained in log {
                    self.keep_frame(arm, retained);
                }
                for record in records {
                    self.keep_record(ArmRecord {
                        arm: arm.to_owned(),
                        record: record.clone(),
                    });
                }
                Some(*seq)
            }
            Event::State(_)
            | Event::Resync { .. }
            | Event::Mode { .. }
            | Event::Rejected { .. }
            | Event::Tripped { .. }
            | Event::ControllerUnavailable { .. }
            | Event::ControllerAvailable => frame.seq,
        };
        if let Some(cursor) = cursor {
            self.resume_after.insert(arm.to_owned(), cursor);
        }
    }
}

pub(crate) struct Shared {
    pub(crate) url: String,
    inner: Mutex<Inner>,
    changed: watch::Sender<u64>,
    run_updates: broadcast::Sender<RunUpdate>,
    action_sequence_updates: watch::Sender<Option<ActionSequenceRecord>>,
    lease_lost: watch::Sender<bool>,
}

impl Shared {
    pub(crate) fn new(url: String, run_after: Option<u64>) -> Self {
        Self {
            url,
            inner: Mutex::new(Inner {
                run_after,
                ..Inner::default()
            }),
            changed: watch::channel(0).0,
            // WHY: Leave room for live events that arrive while the caller finishes connecting.
            run_updates: broadcast::channel(MAX_RUN_REPLAY_RECORDS * 2).0,
            action_sequence_updates: watch::channel(None).0,
            lease_lost: watch::channel(false).0,
        }
    }

    pub(crate) fn subscribe_run(&self) -> broadcast::Receiver<RunUpdate> {
        self.run_updates.subscribe()
    }

    pub(crate) fn subscribe_action_sequence(
        &self,
    ) -> watch::Receiver<Option<ActionSequenceRecord>> {
        self.action_sequence_updates.subscribe()
    }

    pub(crate) fn observation_cursor(&self) -> (u64, u64) {
        self.read(Inner::observation_cursor)
    }

    pub(crate) async fn wait_for_observation(&self, cursor: (u64, u64)) {
        self.wait_for(|inner| (inner.observation_cursor() != cursor).then_some(()))
            .await;
    }

    pub(crate) async fn wait_for_state_event(&self, instance: &str, sequence: u64) {
        self.wait_for(|inner| {
            let state_reflected = inner.state.is_some()
                && inner
                    .event_cursor
                    .as_deref()
                    .and_then(|cursor| cursor.rsplit_once(':'))
                    .is_some_and(|(applied_instance, applied_sequence)| {
                        applied_instance == instance
                            && applied_sequence
                                .parse::<u64>()
                                .is_ok_and(|applied| applied >= sequence)
                    });
            (!inner.is_connected() || state_reflected).then_some(())
        })
        .await;
    }

    pub(crate) async fn wait_for_continuous_disconnect(&self, patience: Duration) {
        loop {
            let since = self
                .wait_for(|inner| inner.stream.disconnected_since())
                .await;
            tokio::time::sleep_until(since + patience).await;
            if self.read(|inner| inner.stream.disconnected_since()) == Some(since) {
                return;
            }
        }
    }

    pub(crate) fn subscribe_lease_lost(&self) -> watch::Receiver<bool> {
        self.lease_lost.subscribe()
    }

    pub(crate) fn reserve_lease_acquisition(&self) -> Option<u64> {
        self.update(|inner| {
            if !inner.is_connected() || !matches!(inner.lease_state, LeaseState::Idle) {
                return None;
            }
            inner.lease_state = LeaseState::Acquiring;
            Some(inner.connection_generation)
        })
    }

    pub(crate) fn finish_lease_acquisition(&self) {
        self.update(|inner| {
            if matches!(inner.lease_state, LeaseState::Acquiring) {
                inner.lease_state = LeaseState::Idle;
            }
        });
    }

    pub(crate) fn install_lease(&self, generation: u64, token: String) -> bool {
        let installed = self.update(|inner| {
            if !inner.is_connected()
                || inner.connection_generation != generation
                || !matches!(inner.lease_state, LeaseState::Acquiring)
            {
                return false;
            }
            inner.lease_state = LeaseState::Held { token };
            inner.lease_loss_status = false;
            true
        });
        if installed {
            self.publish_lease_loss_status();
        }
        installed
    }

    pub(crate) fn lose_lease_if_held_by(&self, token: &str) {
        let lost = self.update(|inner| {
            let lost = inner.lease_state.clear_if_held_by(token);
            inner.lease_loss_status |= lost;
            lost
        });
        if lost {
            self.publish_lease_loss_status();
        }
    }

    fn publish_lease_loss_status(&self) {
        // INVARIANT: Acquire the watch lock with no station lock held. Sample the
        // latest state once publications are serialized so a late publisher cannot
        // overwrite a newer transition with its own snapshot.
        self.lease_lost.send_modify(|published| {
            *published = self.read(|inner| inner.lease_loss_status);
        });
    }

    pub(crate) fn release_lease_if_held_by(&self, token: &str) -> bool {
        self.update(|inner| inner.lease_state.clear_if_held_by(token))
    }

    pub(crate) fn lease_token(&self) -> Option<String> {
        self.read(|inner| inner.lease_state.token().map(str::to_owned))
    }

    pub(crate) fn observes_lease(&self, token: &str) -> Option<bool> {
        self.read(|inner| {
            (inner.lease_state.token() == Some(token))
                .then_some(inner.is_connected() && inner.state.is_some())
        })
    }

    pub(crate) async fn wait_for_lease_loss(&self, token: &str) {
        self.wait_for(|inner| (inner.lease_state.token() != Some(token)).then_some(()))
            .await;
    }

    /// Change the shared state and wake everything waiting on it.
    #[expect(
        clippy::expect_used,
        reason = "Poisoning may indicate an incomplete SDK state update; fail closed rather than reuse uncertain state."
    )]
    pub(crate) fn update<R>(&self, change: impl FnOnce(&mut Inner) -> R) -> R {
        let result = change(&mut self.inner.lock().expect("the state lock is never poisoned"));
        self.changed.send_modify(|revision| *revision += 1);
        result
    }

    #[expect(
        clippy::expect_used,
        reason = "Poisoning may indicate an incomplete SDK state update; fail closed rather than reuse uncertain state."
    )]
    fn read<R>(&self, read: impl FnOnce(&Inner) -> R) -> R {
        read(&self.inner.lock().expect("the state lock is never poisoned"))
    }

    /// Wait until `check` returns a value; it runs again after every change.
    pub(crate) async fn wait_for<R>(&self, mut check: impl FnMut(&Inner) -> Option<R>) -> R {
        let mut changes = self.changed.subscribe();
        loop {
            if let Some(result) = self.read(&mut check) {
                return result;
            }
            if changes.changed().await.is_err() {
                std::future::pending::<()>().await;
            }
        }
    }

    /// Keep the event stream open, reconnecting with its last delivered cursor.
    pub(crate) async fn keep_connected(&self) {
        let mut backoff = hebbian_contracts::Backoff::new(RECONNECT_DELAY_CAP);
        loop {
            let Err(error) = self.serve().await;
            let (was_connected, lease_lost) = self.update(|inner| {
                let was_connected = inner.is_connected();
                if was_connected {
                    inner.connection_generation = inner.connection_generation.wrapping_add(1);
                }
                inner.stream = EventStreamState::Disconnected {
                    since: inner
                        .stream
                        .disconnected_since()
                        .unwrap_or_else(tokio::time::Instant::now),
                    error,
                };
                inner.state = None;
                let lease_lost = inner.lease_state.lose();
                inner.lease_loss_status |= lease_lost;
                (was_connected, lease_lost)
            });
            if lease_lost {
                self.publish_lease_loss_status();
            }
            if was_connected {
                backoff.reset();
            } else {
                tokio::time::sleep(backoff.next_delay()).await;
            }
        }
    }

    async fn serve(&self) -> Result<Infallible, String> {
        let client = reqwest::Client::builder()
            .read_timeout(EVENT_READ_TIMEOUT)
            .build()
            .map_err(|error| error.to_string())?;
        let url = format!("{}/events", self.url.trim_end_matches('/'));
        'snapshot: loop {
            let cursor = self.read(|inner| inner.event_cursor.clone());
            let mut request = client.get(&url);
            if let Some(cursor) = &cursor {
                request = request.header("Last-Event-ID", cursor);
            }
            let response = tokio::time::timeout(HANDSHAKE_TIMEOUT, request.send())
                .await
                .map_err(|error| error.to_string())?
                .map_err(|error| error.to_string())?;
            if response.status() == reqwest::StatusCode::CONFLICT && cursor.is_some() {
                self.update(|inner| inner.event_cursor = None);
                continue;
            }
            let mut response = response
                .error_for_status()
                .map_err(|error| error.to_string())?;
            let mut decoder =
                hebbian_contracts::server_sent_events::ServerSentEventDecoder::default();
            while let Some(chunk) = response.chunk().await.map_err(|error| error.to_string())? {
                decoder.push(&chunk);
                while let Some(event) = decoder.next_event().map_err(|error| error.to_string())? {
                    let update = self
                        .apply_event(
                            &client,
                            &event.kind,
                            event.identifier.as_deref(),
                            &event.data,
                        )
                        .await?;
                    if matches!(update, StreamUpdate::SnapshotRequired) {
                        let retry_without_cursor = self.update(|inner| {
                            inner.event_cursor.take().is_some() && !inner.is_connected()
                        });
                        if retry_without_cursor {
                            continue 'snapshot;
                        }
                        return Err("Station event replay requires a snapshot".into());
                    }
                }
            }
            return Err("station event stream ended".into());
        }
    }

    async fn apply_event(
        &self,
        client: &reqwest::Client,
        kind: &str,
        event_id: Option<&str>,
        data: &str,
    ) -> Result<StreamUpdate, String> {
        if kind == "resync_required" {
            return Ok(StreamUpdate::SnapshotRequired);
        }
        if kind == "snapshot" {
            let snapshot: StationEventSnapshot<Incoming> = serde_json::from_str(data)
                .map_err(|error| format!("Invalid station snapshot: {error}"))?;
            let StationEventSnapshot {
                items,
                run_sequence,
                ..
            } = snapshot;
            let replay_after = self.read(|inner| inner.run_after);
            let records: Vec<Incoming> = if let Some(after) = replay_after {
                let url = format!("{}/run/records", self.url.trim_end_matches('/'));
                client
                    .get(url)
                    .query(&[("after", after)])
                    .timeout(HANDSHAKE_TIMEOUT)
                    .send()
                    .await
                    .map_err(|error| error.to_string())?
                    .error_for_status()
                    .map_err(|error| error.to_string())?
                    .json()
                    .await
                    .map_err(|error| format!("Invalid run replay: {error}"))?
            } else {
                Vec::new()
            };
            // INVARIANT: A replay failure must leave the previous snapshot and cursor intact.
            self.update(|inner| {
                inner.state = None;
                inner.records.clear();
                inner.record_order.clear();
                inner.resume_after.clear();
                inner.run_after.get_or_insert(run_sequence);
            });
            for item in records.into_iter().chain(items) {
                self.receive(item);
            }
        } else if kind == "station" {
            let incoming = serde_json::from_str(data).map_err(|error| error.to_string())?;
            if matches!(incoming, Incoming::Control(StreamControl::ResyncRequired)) {
                return Ok(StreamUpdate::SnapshotRequired);
            }
            self.receive(incoming);
        } else {
            return Ok(StreamUpdate::Continue);
        }
        self.update(|inner| {
            if let Some(event_id) = event_id.filter(|id| !id.is_empty()) {
                inner.event_cursor = Some(event_id.to_owned());
            }
            inner.stream = EventStreamState::Streaming;
        });
        Ok(StreamUpdate::Continue)
    }

    fn receive(&self, incoming: Incoming) {
        let message = match incoming {
            Incoming::Known(message) => message,
            Incoming::Control(_) => return,
            Incoming::Unsupported { kind } => {
                tracing::debug!(kind, "Ignoring unsupported station message");
                return;
            }
        };
        match *message {
            ServerMessage::State { state } => self.update(|inner| {
                inner.state = Some(*state);
                inner.state_count += 1;
            }),
            ServerMessage::ActionSequenceState { action_sequence } => {
                self.action_sequence_updates
                    .send_replace(action_sequence.clone());
                self.update(|inner| {
                    inner.action_sequence = action_sequence;
                    inner.action_sequence_revision += 1;
                });
            }
            ServerMessage::Frame { arm, frame } => {
                self.update(|inner| inner.keep_frame(&arm, &frame));
            }
            ServerMessage::LeaseLost { .. } => {
                self.update(|inner| {
                    inner.lease_state.lose();
                    inner.lease_loss_status = true;
                });
                self.publish_lease_loss_status();
            }
            ServerMessage::RunEvent {
                seq,
                received_unix_nano,
                event,
            } => self.run_record(
                seq,
                RunUpdate::Event {
                    seq,
                    received_unix_nano,
                    event,
                },
            ),
            ServerMessage::RunSpan {
                seq,
                received_unix_nano,
                span,
            } => self.run_record(
                seq,
                RunUpdate::Span {
                    seq,
                    received_unix_nano,
                    span: Box::new(span),
                },
            ),
            ServerMessage::RunResync {
                oldest_retained,
                newest,
            } => {
                self.update(|inner| inner.run_after = Some(newest));
                let _ = self.run_updates.send(RunUpdate::Resync {
                    oldest_retained,
                    newest,
                });
            }
            ServerMessage::Run { run } => {
                let _ = self.run_updates.send(RunUpdate::Snapshot(run));
            }
            ServerMessage::Hello { .. }
            | ServerMessage::ActionSequenceAccepted { .. }
            | ServerMessage::Lease { .. }
            | ServerMessage::CommandAnswer { .. }
            | ServerMessage::PreviewAnswer { .. }
            | ServerMessage::Refused { .. } => {}
        }
    }

    /// Deliver a run record once, in order, after a resume or a duplicate.
    fn run_record(&self, seq: u64, update: RunUpdate) {
        let fresh = self.update(|inner| {
            let fresh = inner.run_after.is_none_or(|previous| seq > previous);
            if fresh {
                inner.run_after = Some(seq);
            }
            fresh
        });
        if fresh {
            let _ = self.run_updates.send(update);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{EventStreamState, Shared};
    use std::time::Duration;

    #[tokio::test]
    async fn lease_loss_is_visible_while_a_subscriber_holds_the_previous_notification()
    -> Result<(), Box<dyn std::error::Error>> {
        let shared = Shared::new(String::new(), None);
        shared.update(|inner| inner.stream = EventStreamState::Streaming);
        let generation = shared
            .reserve_lease_acquisition()
            .ok_or("reservation refused")?;
        let token = "held";
        assert!(shared.install_lease(generation, token.into()));
        let notifications = shared.subscribe_lease_lost();
        let notification = notifications.borrow();
        let runtime = tokio::runtime::Handle::current();
        let (loss_observed, observed) = std::sync::mpsc::channel();
        let observed_while_borrowed = std::thread::scope(|threads| {
            threads.spawn(|| shared.lose_lease_if_held_by(token));
            let shared = &shared;
            threads.spawn(move || {
                runtime.block_on(shared.wait_for_lease_loss(token));
                let _ = loss_observed.send(());
            });
            let observed_while_borrowed = observed.recv_timeout(Duration::from_secs(2));
            drop(notification);
            observed_while_borrowed
        });
        assert!(
            observed_while_borrowed.is_ok(),
            "lease state was blocked by a notification reader"
        );
        assert!(*notifications.borrow());
        Ok(())
    }
}

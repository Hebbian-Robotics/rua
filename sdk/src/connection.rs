//! The resumable HTTP event stream and its cached station observations.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::Mutex;
use std::time::Duration;

use hebbian_contracts::{
    ActionSequenceRecord, ArmRecord, Event, Frame, LiveRun, RunEvent, ServerMessage,
    StationEventSnapshot, StationState, TraceSpan, UnixNanos,
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
    /// A kind this SDK does not know: skipped, so a newer station still works.
    Unknown(#[allow(dead_code, reason = "only to accept any JSON")] serde::de::IgnoredAny),
}

#[derive(serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum StreamControl {
    ResyncRequired,
}

#[derive(Default)]
pub(crate) enum LeaseState {
    #[default]
    Idle,
    Acquiring,
    Held {
        token: String,
    },
}

impl LeaseState {
    pub(crate) fn token(&self) -> Option<&str> {
        match self {
            Self::Held { token } => Some(token),
            Self::Idle | Self::Acquiring => None,
        }
    }

    pub(crate) fn clear_if_held_by(&mut self, token: &str) -> bool {
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
pub(crate) struct Inner {
    pub(crate) connected: bool,
    pub(crate) connection_generation: u64,
    event_cursor: Option<String>,
    pub(crate) disconnected_since: Option<tokio::time::Instant>,
    pub(crate) last_error: Option<String>,
    pub(crate) state: Option<StationState>,
    /// State frames received so far, so a waiter can tell a frame that arrived after
    /// some event from the cached one.
    pub(crate) state_count: u64,
    pub(crate) action_sequence: Option<ActionSequenceRecord>,
    pub(crate) action_sequence_revision: u64,
    records: HashMap<String, ArmRecord>,
    record_order: VecDeque<String>,
    pub(crate) lease_state: LeaseState,
    resume_after: BTreeMap<String, u64>,
    run_after: Option<u64>,
}

impl Inner {
    pub(crate) fn record(&self, command_id: &str) -> Option<&ArmRecord> {
        self.records.get(command_id)
    }

    pub(crate) fn keep_record(&mut self, arm_record: ArmRecord) {
        let record = &arm_record.record;
        // A late, unfinished revision never replaces the finished one.
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
            | Event::Reply { .. }
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
    pub(crate) action_sequence_updates: watch::Sender<Option<ActionSequenceRecord>>,
    pub(crate) lease_lost: watch::Sender<bool>,
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
            run_updates: broadcast::channel(1024).0,
            action_sequence_updates: watch::channel(None).0,
            lease_lost: watch::channel(false).0,
        }
    }

    pub(crate) fn subscribe_run(&self) -> broadcast::Receiver<RunUpdate> {
        self.run_updates.subscribe()
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
    pub(crate) fn read<R>(&self, read: impl FnOnce(&Inner) -> R) -> R {
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
            let ended = self.serve().await;
            let was_connected = self.update(|inner| {
                let was_connected = inner.connected;
                if was_connected {
                    inner.connection_generation = inner.connection_generation.wrapping_add(1);
                }
                inner.connected = false;
                inner.state = None;
                inner
                    .disconnected_since
                    .get_or_insert_with(tokio::time::Instant::now);
                if let Err(error) = &ended {
                    inner.last_error = Some(error.clone());
                }
                if inner.lease_state.lose() {
                    self.lease_lost.send_replace(true);
                }
                was_connected
            });
            if was_connected {
                backoff.reset();
            } else {
                tokio::time::sleep(backoff.next_delay()).await;
            }
        }
    }

    async fn serve(&self) -> Result<(), String> {
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
                            inner.event_cursor.take().is_some() && !inner.connected
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
            let replay_after = self.update(|inner| {
                inner.state = None;
                inner.records.clear();
                inner.record_order.clear();
                inner.resume_after.clear();
                let previous = inner.run_after;
                if previous.is_none() {
                    inner.run_after = Some(run_sequence);
                }
                previous
            });
            if let Some(after) = replay_after {
                let url = format!("{}/run/records", self.url.trim_end_matches('/'));
                let records: Vec<Incoming> = client
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
                    .map_err(|error| error.to_string())?;
                for record in records {
                    self.receive(record);
                }
            }
            for item in items {
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
            inner.connected = true;
            inner.disconnected_since = None;
            inner.last_error = None;
        });
        Ok(StreamUpdate::Continue)
    }

    fn receive(&self, incoming: Incoming) {
        let Incoming::Known(message) = incoming else {
            return;
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
                });
                self.lease_lost.send_replace(true);
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

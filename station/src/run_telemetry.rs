use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard};

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use hebbian_contracts::{
    KnownRunEvent, LiveRun, MAX_RUN_REPLAY_RECORDS, RunEvent, RunMemory, RunUsage, ServerMessage,
    TraceSpan, UnixNanos,
};
use hebbian_rua_core::station_api::{RunReplayWindow, RunTelemetry, RunUpdates};

const KEPT_SPANS: usize = 1000;

#[derive(Debug, Clone, PartialEq)]
enum RunRecordKind {
    Event(RunEvent),
    Span(TraceSpan),
}

#[derive(Debug, Clone, PartialEq)]
struct RunRecord {
    seq: u64,
    received_unix_nano: UnixNanos,
    kind: RunRecordKind,
}

#[derive(Debug, Default)]
struct RunBuffer {
    revision: u64,
    next_seq: u64,
    records: VecDeque<RunRecord>,
    spans: VecDeque<TraceSpan>,
    usage: Option<RunUsage>,
    memory: RunMemory,
}

impl RunBuffer {
    fn newest_seq(&self) -> u64 {
        self.next_seq.saturating_sub(1)
    }

    fn replay_window(&self) -> RunReplayWindow {
        RunReplayWindow {
            oldest_retained: self.records.front().map_or(1, |record| record.seq),
            newest: self.newest_seq(),
        }
    }

    fn log(&mut self, kind: RunRecordKind) {
        self.next_seq = self.next_seq.max(1);
        if self.records.len() >= MAX_RUN_REPLAY_RECORDS {
            self.records.pop_front();
        }
        self.records.push_back(RunRecord {
            seq: self.next_seq,
            received_unix_nano: UnixNanos::now(),
            kind,
        });
        self.next_seq += 1;
    }

    // INVARIANT: A cursor outside the retained range requires resynchronization, including a
    // cursor ahead of the newest record after a restart.
    fn since(&self, after: u64) -> Result<Vec<RunRecord>, RunReplayWindow> {
        let window = self.replay_window();
        if after > window.newest || after.saturating_add(1) < window.oldest_retained {
            return Err(window);
        }
        Ok(self
            .records
            .iter()
            .filter(|record| record.seq > after)
            .cloned()
            .collect())
    }

    fn record_span(&mut self, span: TraceSpan) {
        self.log(RunRecordKind::Span(span.clone()));
        if let Some(index) = self
            .spans
            .iter()
            .position(|kept| kept.trace_id == span.trace_id && kept.span_id == span.span_id)
        {
            self.spans.remove(index);
        }
        self.spans.push_back(span);
        while self.spans.len() > KEPT_SPANS {
            self.spans.pop_front();
        }
        self.revision += 1;
    }

    fn record_event(&mut self, event: RunEvent) {
        self.log(RunRecordKind::Event(event.clone()));
        match event {
            RunEvent::Known(KnownRunEvent::RunStarted { task }) => {
                self.usage = None;
                self.memory = RunMemory {
                    task,
                    ..RunMemory::default()
                };
                // WHY: Keep the run's root span, which the tracer sent as it started.
                let root = self
                    .spans
                    .iter()
                    .filter(|span| span.is_active() && span.is_run())
                    .max_by_key(|span| span.start_time_unix_nano)
                    .cloned();
                self.spans = root.into_iter().collect();
            }
            RunEvent::Known(KnownRunEvent::PlanUpdate { steps }) => {
                self.memory.plan = Some(steps);
            }
            RunEvent::Known(KnownRunEvent::NotesUpdate { notes }) => {
                self.memory.notes = notes;
            }
            RunEvent::Known(KnownRunEvent::UsageUpdate { usage }) => {
                self.usage = Some(usage);
            }
            RunEvent::Other(_) => {}
        }
        self.revision += 1;
    }

    fn snapshot(&self) -> LiveRun {
        let now = UnixNanos::now();
        LiveRun {
            revision: self.revision,
            spans: self
                .spans
                .iter()
                .map(|span| {
                    let mut span = span.clone();
                    if span.is_active() {
                        span.end_time_unix_nano = now;
                    }
                    span
                })
                .collect(),
            usage: self.usage.clone(),
            memory: self.memory.clone(),
        }
    }

    fn messages_after(&self, cursor: &mut u64) -> Vec<ServerMessage> {
        let newest = self.newest_seq();
        let records = match self.since(*cursor) {
            Ok(records) => records,
            Err(gap) => {
                *cursor = newest;
                return vec![ServerMessage::RunResync {
                    oldest_retained: gap.oldest_retained,
                    newest: gap.newest,
                }];
            }
        };
        *cursor = newest;
        records
            .into_iter()
            .map(|record| match record.kind {
                RunRecordKind::Event(event) => ServerMessage::RunEvent {
                    seq: record.seq,
                    received_unix_nano: record.received_unix_nano,
                    event,
                },
                RunRecordKind::Span(span) => ServerMessage::RunSpan {
                    seq: record.seq,
                    received_unix_nano: record.received_unix_nano,
                    span,
                },
            })
            .collect()
    }
}

#[derive(Debug, Default)]
pub struct LiveRunStore {
    buffer: Mutex<RunBuffer>,
}

impl LiveRunStore {
    fn lock(&self) -> MutexGuard<'_, RunBuffer> {
        self.buffer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    #[must_use]
    pub fn snapshot(&self) -> LiveRun {
        self.lock().snapshot()
    }
}

impl RunTelemetry for LiveRunStore {
    fn record_event(&self, event: RunEvent) {
        self.lock().record_event(event);
    }

    fn record_spans(&self, spans: Vec<TraceSpan>) {
        let mut buffer = self.lock();
        for span in spans {
            buffer.record_span(span);
        }
    }

    fn replay_window(&self) -> RunReplayWindow {
        self.lock().replay_window()
    }

    fn updates(&self, cursor: &mut u64, revision_sent: &mut Option<u64>) -> RunUpdates {
        let buffer = self.lock();
        let records = buffer.messages_after(cursor);
        let snapshot = (*revision_sent != Some(buffer.revision)).then(|| buffer.snapshot());
        if let Some(run) = &snapshot {
            *revision_sent = Some(run.revision);
        }
        RunUpdates { records, snapshot }
    }
}

pub fn routes(store: Arc<LiveRunStore>) -> Router {
    Router::new()
        .route(
            "/run",
            get(|State(store): State<Arc<LiveRunStore>>| async move { Json(store.snapshot()) }),
        )
        .with_state(store)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(id: &str, attributes: &serde_json::Value) -> TraceSpan {
        serde_json::from_value(serde_json::json!({
            "trace_id": "t", "span_id": id, "name": id,
            "start_time_unix_nano": "1", "end_time_unix_nano": "2",
            "attributes": attributes,
        }))
        .unwrap()
    }

    fn event(value: serde_json::Value) -> RunEvent {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn a_new_run_keeps_only_its_root_span_and_clears_the_memory() {
        let mut buffer = RunBuffer::default();
        buffer.record_span(span("old", &serde_json::json!({})));
        buffer.record_span(span(
            "root",
            &serde_json::json!({"hebbian.active": true, "hebbian.kind": "run"}),
        ));
        buffer.record_event(event(
            serde_json::json!({"type": "notes_update", "notes": ["a"]}),
        ));
        buffer.record_event(event(
            serde_json::json!({"type": "run_started", "task": "stack"}),
        ));
        let run = buffer.snapshot();
        assert_eq!(run.spans.len(), 1);
        assert_eq!(run.spans[0].span_id, "root");
        assert!(
            run.spans[0].end_time_unix_nano.0 > 2,
            "a running span ends now"
        );
        assert_eq!(run.memory.task.as_deref(), Some("stack"));
        assert_eq!(run.memory.notes, Vec::<String>::new());
        let before = buffer.revision;
        buffer.record_event(event(serde_json::json!({"type": "model_turn", "turn": {}})));
        assert_eq!(buffer.revision, before + 1);
        assert_eq!(
            serde_json::to_value(buffer.snapshot().spans[0].start_time_unix_nano).unwrap(),
            "1"
        );
    }

    #[test]
    fn a_client_resumes_run_records_and_is_told_of_gaps() {
        let store = LiveRunStore::default();
        store.record_event(event(
            serde_json::json!({"type": "notes_update", "notes": ["a"]}),
        ));
        let seen = store.replay_window().newest;
        store.record_spans(vec![span("s", &serde_json::json!({}))]);

        let mut cursor = seen;
        let mut revision_sent = None;
        let resumed = store.updates(&mut cursor, &mut revision_sent);
        assert_eq!(resumed.records.len(), 1);
        let ServerMessage::RunSpan { seq, span, .. } = &resumed.records[0] else {
            panic!("expected a run span, got {:?}", resumed.records[0]);
        };
        assert_eq!((*seq, span.span_id.as_str()), (seen + 1, "s"));
        assert_eq!(cursor, seen + 1);
        let snapshot = resumed.snapshot.expect("the first update carries the run");
        assert_eq!(snapshot.memory.notes, ["a"]);
        let quiet = store.updates(&mut cursor, &mut revision_sent);
        assert!(quiet.records.is_empty() && quiet.snapshot.is_none());

        let mut ahead = 1_000_000;
        let gap = store.updates(&mut ahead, &mut None);
        assert!(matches!(
            gap.records[..],
            [ServerMessage::RunResync { newest, .. }] if newest == seen + 1
        ));
        assert_eq!(ahead, seen + 1);
    }

    #[test]
    fn the_buffer_keeps_only_the_newest_records() {
        let mut buffer = RunBuffer::default();
        for index in 0..MAX_RUN_REPLAY_RECORDS + 10 {
            buffer.record_span(span(&index.to_string(), &serde_json::json!({})));
        }
        assert_eq!(buffer.records.len(), MAX_RUN_REPLAY_RECORDS);
        assert!(buffer.since(0).is_err());
        assert_eq!(buffer.since(buffer.newest_seq() - 1).unwrap().len(), 1);
        assert_eq!(buffer.spans.len(), MAX_RUN_REPLAY_RECORDS.min(KEPT_SPANS));
    }
}

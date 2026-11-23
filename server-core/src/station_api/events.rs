use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};

use hebbian_contracts::{Event, Frame, ServerMessage};
use serde::Serialize;
use tokio::sync::broadcast;

const RETAINED_EVENTS: usize = 4096;
const RETAINED_ARM_LOG_FRAMES: usize = 4096;

#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub(super) enum StreamPayload {
    Message(ServerMessage),
    Notice(StreamNotice),
}

impl StreamPayload {
    pub(super) fn is_station_state(&self) -> bool {
        matches!(self, Self::Message(ServerMessage::State { .. }))
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(super) enum StreamNotice {
    ResyncRequired {
        #[serde(skip_serializing_if = "Option::is_none")]
        arm: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        detail: Option<&'static str>,
    },
}

#[derive(Debug, Clone)]
pub(super) struct StationEvent {
    pub sequence: u64,
    pub payload: Arc<StreamPayload>,
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum SnapshotKey {
    ArmState(String),
    ArmSnapshot(String),
    StationState,
    Run,
    ActionSequence(Vec<String>),
}

struct JournalState {
    next_sequence: u64,
    events: VecDeque<StationEvent>,
    snapshots: BTreeMap<SnapshotKey, StationEvent>,
    records: VecDeque<((String, String), StationEvent)>,
}

// INVARIANT: Events and subscription are captured under one lock to avoid replay gaps.
pub(super) struct EventJournal {
    state: Mutex<JournalState>,
    updates: broadcast::Sender<StationEvent>,
}

// CONSTRAINT: A cursor outside retained history requires a new snapshot.
pub(super) enum EventSubscription {
    Replay {
        events: Vec<StationEvent>,
        updates: broadcast::Receiver<StationEvent>,
    },
    ResyncRequired,
}

impl Default for EventJournal {
    fn default() -> Self {
        let (updates, _) = broadcast::channel(RETAINED_EVENTS);
        Self {
            state: Mutex::new(JournalState {
                next_sequence: 1,
                events: VecDeque::new(),
                snapshots: BTreeMap::new(),
                records: VecDeque::new(),
            }),
            updates,
        }
    }
}

impl EventJournal {
    pub fn publish(&self, message: ServerMessage) {
        self.publish_payload(StreamPayload::Message(message));
    }

    pub fn resync_arm(&self, arm: String) {
        self.publish_payload(StreamPayload::Notice(StreamNotice::ResyncRequired {
            arm: Some(arm),
            detail: None,
        }));
    }

    fn publish_payload(&self, payload: StreamPayload) {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .publish(payload, &self.updates);
    }

    pub fn invalidate_station_state(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.snapshots.remove(&SnapshotKey::StationState).is_some() {
            state.publish(
                StreamPayload::Notice(StreamNotice::ResyncRequired {
                    arm: None,
                    detail: Some("Station state is unavailable"),
                }),
                &self.updates,
            );
        }
    }

    // INVARIANT: Capture the latest retained state and start live delivery atomically.
    pub fn subscribe_with_snapshot(
        &self,
    ) -> (
        u64,
        Vec<Arc<StreamPayload>>,
        broadcast::Receiver<StationEvent>,
    ) {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut snapshots = state
            .snapshots
            .values()
            .chain(state.records.iter().map(|(_, event)| event))
            .collect::<Vec<_>>();
        snapshots.sort_unstable_by_key(|event| event.sequence);
        (
            state.next_sequence - 1,
            snapshots
                .into_iter()
                .map(|event| Arc::clone(&event.payload))
                .collect(),
            self.updates.subscribe(),
        )
    }

    pub fn subscribe_after(&self, cursor: u64) -> EventSubscription {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let newest = state.next_sequence - 1;
        let oldest = state
            .events
            .front()
            .map_or(state.next_sequence, |event| event.sequence);
        if cursor > newest || cursor.saturating_add(1) < oldest {
            return EventSubscription::ResyncRequired;
        }
        let updates = self.updates.subscribe();
        let events = state
            .events
            .iter()
            .filter(|event| event.sequence > cursor)
            .cloned()
            .collect();
        EventSubscription::Replay { events, updates }
    }
}

impl JournalState {
    #[expect(
        clippy::expect_used,
        reason = "A u64 event counter cannot exhaust in practice; wrapping would make replay cursors ambiguous."
    )]
    fn publish(&mut self, payload: StreamPayload, updates: &broadcast::Sender<StationEvent>) {
        let event = StationEvent {
            sequence: self.next_sequence,
            payload: Arc::new(payload),
        };
        self.next_sequence = self
            .next_sequence
            .checked_add(1)
            .expect("station event sequence exhausted");
        if let StreamPayload::Message(message) = event.payload.as_ref() {
            let key = match message {
                ServerMessage::Frame { arm, frame } => self.retain_arm_frame(arm, frame, &event),
                ServerMessage::State { .. } => Some(SnapshotKey::StationState),
                ServerMessage::Run { .. } => Some(SnapshotKey::Run),
                ServerMessage::ActionSequenceState {
                    action_sequence: Some(record),
                } => Some(SnapshotKey::ActionSequence(record.arms.clone())),
                ServerMessage::ActionSequenceAccepted { .. }
                | ServerMessage::Hello { .. }
                | ServerMessage::Lease { .. }
                | ServerMessage::CommandAnswer { .. }
                | ServerMessage::PreviewAnswer { .. }
                | ServerMessage::LeaseLost { .. }
                | ServerMessage::ActionSequenceState { .. }
                | ServerMessage::RunEvent { .. }
                | ServerMessage::RunSpan { .. }
                | ServerMessage::RunResync { .. }
                | ServerMessage::Refused { .. } => None,
            };
            if let Some(key) = key {
                self.snapshots.insert(key, event.clone());
            }
        }
        self.events.push_back(event.clone());
        if self.events.len() > RETAINED_EVENTS {
            self.events.pop_front();
        }
        let _ = updates.send(event);
    }

    fn retain_arm_frame(
        &mut self,
        arm: &str,
        frame: &Frame,
        event: &StationEvent,
    ) -> Option<SnapshotKey> {
        match &frame.event {
            Event::State(_) => return Some(SnapshotKey::ArmState(arm.to_owned())),
            Event::Snapshot { .. } => {
                self.records
                    .retain(|((record_arm, _), _)| record_arm != arm);
                return Some(SnapshotKey::ArmSnapshot(arm.to_owned()));
            }
            Event::Record(record) => {
                self.records.retain(|((record_arm, command), _)| {
                    record_arm != arm || command != &record.command_id
                });
                self.records
                    .push_back(((arm.to_owned(), record.command_id.clone()), event.clone()));
                if self.records.len() > RETAINED_EVENTS {
                    self.records.pop_front();
                }
            }
            Event::Resync { .. }
            | Event::Mode { .. }
            | Event::Rejected { .. }
            | Event::Tripped { .. }
            | Event::ControllerUnavailable { .. }
            | Event::ControllerAvailable => {}
        }
        self.refresh_arm_snapshot(arm, frame);
        None
    }

    // WHY: Refresh the retained snapshot from the sequenced log so later clients see controller recovery.
    fn refresh_arm_snapshot(&mut self, arm: &str, frame: &Frame) {
        let Some(frame_sequence) = frame.seq else {
            return;
        };
        let Some(snapshot) = self
            .snapshots
            .get_mut(&SnapshotKey::ArmSnapshot(arm.to_owned()))
        else {
            return;
        };
        let StreamPayload::Message(ServerMessage::Frame {
            frame: snapshot, ..
        }) = Arc::make_mut(&mut snapshot.payload)
        else {
            return;
        };
        let Event::Snapshot {
            seq,
            mode,
            fault,
            log,
            ..
        } = &mut snapshot.event
        else {
            return;
        };
        if let Event::Mode {
            mode: new_mode,
            fault: new_fault,
        } = &frame.event
        {
            *mode = *new_mode;
            fault.clone_from(new_fault);
        }
        log.push(frame.clone());
        if log.len() > RETAINED_ARM_LOG_FRAMES {
            log.remove(0);
        }
        *seq = frame_sequence;
    }
}

#[cfg(test)]
mod tests {
    use super::{Event, EventJournal, EventSubscription, Frame, RETAINED_EVENTS, ServerMessage};
    use hebbian_contracts::{ArmMode, CommandRecord, Sample};

    fn frame(journal: &EventJournal, arm: &str, seq: Option<u64>, event: Event) {
        journal.publish(ServerMessage::Frame {
            arm: arm.to_owned(),
            frame: Box::new(Frame::emitted_now(seq, event)),
        });
    }

    fn snapshot(seq: u64, mode: ArmMode, fault: Option<String>, log: Vec<Frame>) -> Event {
        Event::Snapshot {
            seq,
            mode,
            fault,
            log,
            sample: None,
            records: vec![],
        }
    }

    fn record(command_id: &str) -> Event {
        Event::Record(CommandRecord::new(
            command_id.to_owned(),
            "stop",
            0.0,
            hebbian_contracts::RecordedCommand::Control {
                action: hebbian_contracts::ControlAction::Stop,
            },
        ))
    }

    fn snapshot_items(journal: &EventJournal) -> (u64, Vec<serde_json::Value>) {
        let (cursor, items, _) = journal.subscribe_with_snapshot();
        (
            cursor,
            items
                .iter()
                .map(|item| serde_json::to_value(item.as_ref()).unwrap())
                .collect(),
        )
    }
    #[test]
    fn snapshot_orders_retained_state_and_discards_records_replaced_by_arm_snapshot() {
        let journal = EventJournal::default();
        frame(&journal, "left", None, record("old"));
        frame(
            &journal,
            "left",
            None,
            snapshot(0, ArmMode::Disabled, None, vec![]),
        );
        frame(&journal, "left", None, Event::State(Sample::blank()));
        frame(&journal, "left", None, record("new"));
        let (cursor, snapshot) = snapshot_items(&journal);
        assert_eq!(cursor, 4);
        assert_eq!(snapshot.len(), 3);
        assert_eq!(snapshot[0]["frame"]["event"]["kind"], "snapshot");
        assert_eq!(snapshot[1]["frame"]["event"]["kind"], "state");
        assert_eq!(snapshot[2]["frame"]["event"]["command_id"], "new");
    }

    #[test]
    fn a_later_logged_event_reaches_clients_that_join_after_it() {
        let journal = EventJournal::default();
        frame(
            &journal,
            "left",
            None,
            snapshot(
                1,
                ArmMode::Disabled,
                None,
                vec![Frame::emitted_now(
                    Some(1),
                    Event::ControllerUnavailable {
                        reason: "lost".into(),
                    },
                )],
            ),
        );
        frame(&journal, "left", Some(2), Event::ControllerAvailable);
        frame(&journal, "left", None, Event::State(Sample::blank()));

        let (_, snapshot) = snapshot_items(&journal);
        let arm_snapshot = &snapshot[0]["frame"]["event"];
        assert_eq!(arm_snapshot["seq"], 2);
        let log = arm_snapshot["log"].as_array().unwrap();
        assert_eq!(log.len(), 2);
        assert_eq!(log[1]["event"]["kind"], "controller_available");
    }

    #[test]
    fn a_later_mode_change_replaces_the_snapshots_mode_and_clears_its_fault() {
        let journal = EventJournal::default();
        frame(
            &journal,
            "left",
            None,
            snapshot(1, ArmMode::Faulted, Some("stale".to_owned()), vec![]),
        );
        frame(
            &journal,
            "left",
            Some(2),
            Event::Mode {
                mode: ArmMode::Disabled,
                fault: None,
            },
        );

        let (_, snapshot) = snapshot_items(&journal);
        let arm_snapshot = &snapshot[0]["frame"]["event"];
        assert_eq!(arm_snapshot["mode"], "disabled");
        assert!(arm_snapshot["fault"].is_null());
    }

    #[test]
    fn snapshot_preserves_records_for_other_arms_with_shared_name_prefixes() {
        let journal = EventJournal::default();
        frame(&journal, "left:tool", None, record("move"));
        frame(&journal, "left", None, record("tool:move"));
        frame(
            &journal,
            "left",
            None,
            snapshot(0, ArmMode::Disabled, None, vec![]),
        );

        let (_, snapshot) = snapshot_items(&journal);
        assert_eq!(snapshot.len(), 2);
        assert_eq!(snapshot[0]["arm"], "left:tool");
        assert_eq!(snapshot[0]["frame"]["event"]["command_id"], "move");
        assert_eq!(snapshot[1]["arm"], "left");
        assert_eq!(snapshot[1]["frame"]["event"]["kind"], "snapshot");
    }

    #[test]
    fn replay_and_live_delivery_have_no_gap_and_old_cursors_require_resync() {
        let journal = EventJournal::default();
        journal.resync_arm("left".to_owned());
        let EventSubscription::Replay {
            events,
            mut updates,
        } = journal.subscribe_after(0)
        else {
            panic!("retained cursor must replay");
        };
        journal.resync_arm("left".to_owned());
        assert_eq!(
            events
                .iter()
                .map(|event| event.sequence)
                .collect::<Vec<_>>(),
            [1]
        );
        assert_eq!(updates.try_recv().unwrap().sequence, 2);
        for _ in 0..RETAINED_EVENTS {
            journal.resync_arm("left".to_owned());
        }
        assert!(matches!(
            journal.subscribe_after(0),
            EventSubscription::ResyncRequired
        ));
        assert!(matches!(
            journal.subscribe_after(u64::MAX),
            EventSubscription::ResyncRequired
        ));
    }
}

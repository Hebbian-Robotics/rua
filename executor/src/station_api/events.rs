use std::collections::{BTreeMap, VecDeque};
use std::sync::Mutex;

use tokio::sync::broadcast;

const RETAINED_EVENTS: usize = 4096;
const RETAINED_ARM_LOG_FRAMES: usize = 4096;

#[derive(Debug, Clone, PartialEq)]
pub struct StationEvent {
    pub sequence: u64,
    pub payload: serde_json::Value,
}

struct JournalState {
    next_sequence: u64,
    events: VecDeque<StationEvent>,
    snapshots: BTreeMap<String, (u64, serde_json::Value)>,
    records: VecDeque<((String, String), u64, serde_json::Value)>,
}

// INVARIANT: Events and subscription are captured under one lock to avoid replay gaps.
pub struct EventJournal {
    state: Mutex<JournalState>,
    updates: broadcast::Sender<StationEvent>,
}

// CONSTRAINT: A cursor outside retained history requires a new snapshot.
pub enum EventSubscription {
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
    #[expect(
        clippy::expect_used,
        reason = "A u64 event counter cannot exhaust in practice; wrapping would make replay cursors ambiguous."
    )]
    pub fn publish(&self, payload: serde_json::Value) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if payload
            .pointer("/frame/event/kind")
            .and_then(serde_json::Value::as_str)
            == Some("record")
            && let Some(id) = payload
                .pointer("/frame/event/command_id")
                .and_then(serde_json::Value::as_str)
        {
            let record_identity = (
                payload
                    .get("arm")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                id.to_owned(),
            );
            let sequence = state.next_sequence;
            state
                .records
                .retain(|(existing, _, _)| existing != &record_identity);
            state
                .records
                .push_back((record_identity, sequence, payload.clone()));
            if state.records.len() > RETAINED_EVENTS {
                state.records.pop_front();
            }
        }
        let snapshot_key = match payload.get("kind").and_then(serde_json::Value::as_str) {
            Some("frame")
                if payload
                    .pointer("/frame/event/kind")
                    .and_then(serde_json::Value::as_str)
                    == Some("state") =>
            {
                payload
                    .get("arm")
                    .and_then(serde_json::Value::as_str)
                    .map(|arm| format!("arm:{arm}"))
            }
            Some("frame")
                if payload
                    .pointer("/frame/event/kind")
                    .and_then(serde_json::Value::as_str)
                    == Some("snapshot") =>
            {
                payload
                    .get("arm")
                    .and_then(serde_json::Value::as_str)
                    .map(|arm| format!("snapshot:{arm}"))
            }
            Some("state") => Some("station_state".to_owned()),
            Some("run") => Some("run".to_owned()),
            Some("action_sequence_state") => payload
                .pointer("/action_sequence/arms")
                .and_then(serde_json::Value::as_array)
                .map(|arms| {
                    format!(
                        "sequence:{}",
                        arms.iter()
                            .filter_map(serde_json::Value::as_str)
                            .collect::<Vec<_>>()
                            .join(",")
                    )
                }),
            _ => None,
        };
        fold_logged_frame_into_arm_snapshot(&mut state.snapshots, &payload);
        if let Some(key) = snapshot_key {
            if let Some(arm) = key.strip_prefix("snapshot:") {
                state
                    .records
                    .retain(|((record_arm, _), _, _)| record_arm != arm);
            }
            let sequence = state.next_sequence;
            state.snapshots.insert(key, (sequence, payload.clone()));
        }
        let event = StationEvent {
            sequence: state.next_sequence,
            payload,
        };
        state.next_sequence = state
            .next_sequence
            .checked_add(1)
            .expect("station event sequence exhausted");
        state.events.push_back(event.clone());
        if state.events.len() > RETAINED_EVENTS {
            state.events.pop_front();
        }
        let _ = self.updates.send(event);
    }

    pub fn invalidate_station_state(&self) {
        let removed = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .snapshots
            .remove("station_state")
            .is_some();
        if removed {
            self.publish(serde_json::json!({"kind": "resync_required", "detail": "Station state is unavailable"}));
        }
    }

    // INVARIANT: Capture the latest retained state and start live delivery atomically.
    pub fn subscribe_with_snapshot(
        &self,
    ) -> (
        u64,
        Vec<serde_json::Value>,
        broadcast::Receiver<StationEvent>,
    ) {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut snapshots = state
            .snapshots
            .values()
            .cloned()
            .chain(
                state
                    .records
                    .iter()
                    .map(|(_, sequence, payload)| (*sequence, payload.clone())),
            )
            .collect::<Vec<_>>();
        snapshots.sort_unstable_by_key(|(sequence, _)| *sequence);
        (
            state.next_sequence - 1,
            snapshots.into_iter().map(|(_, payload)| payload).collect(),
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

// WHY: Refresh the retained snapshot from the sequenced log so later clients see controller recovery rather than the one-time startup state.
fn fold_logged_frame_into_arm_snapshot(
    snapshots: &mut BTreeMap<String, (u64, serde_json::Value)>,
    payload: &serde_json::Value,
) {
    if payload.get("kind").and_then(serde_json::Value::as_str) != Some("frame") {
        return;
    }
    let event_kind = payload
        .pointer("/frame/event/kind")
        .and_then(serde_json::Value::as_str);
    if matches!(event_kind, Some("state" | "snapshot")) {
        return;
    }
    let (Some(arm), Some(frame_sequence), Some(frame)) = (
        payload.get("arm").and_then(serde_json::Value::as_str),
        payload
            .pointer("/frame/seq")
            .and_then(serde_json::Value::as_u64),
        payload.get("frame"),
    ) else {
        return;
    };
    let Some((_, snapshot)) = snapshots.get_mut(&format!("snapshot:{arm}")) else {
        return;
    };
    if event_kind == Some("mode") {
        for field in ["mode", "fault"] {
            snapshot["frame"]["event"][field] = payload
                .pointer(&format!("/frame/event/{field}"))
                .cloned()
                .unwrap_or(serde_json::Value::Null);
        }
    }
    let Some(retained_log) = snapshot
        .pointer_mut("/frame/event/log")
        .and_then(serde_json::Value::as_array_mut)
    else {
        return;
    };
    retained_log.push(frame.clone());
    if retained_log.len() > RETAINED_ARM_LOG_FRAMES {
        retained_log.remove(0);
    }
    snapshot["frame"]["event"]["seq"] = frame_sequence.into();
}

#[cfg(test)]
mod tests {
    use super::{EventJournal, EventSubscription, RETAINED_EVENTS};

    #[test]
    fn snapshot_orders_retained_state_and_discards_records_replaced_by_arm_snapshot() {
        let journal = EventJournal::default();
        journal.publish(serde_json::json!({"kind":"frame", "arm":"left", "frame":{"event":{"kind":"record", "command_id":"old"}}}));
        journal.publish(serde_json::json!({"kind":"frame", "arm":"left", "frame":{"event":{"kind":"snapshot"}}}));
        journal.publish(
            serde_json::json!({"kind":"frame", "arm":"left", "frame":{"event":{"kind":"state"}}}),
        );
        journal.publish(serde_json::json!({"kind":"frame", "arm":"left", "frame":{"event":{"kind":"record", "command_id":"new"}}}));
        let (cursor, snapshot, _) = journal.subscribe_with_snapshot();
        assert_eq!(cursor, 4);
        assert_eq!(snapshot.len(), 3);
        assert_eq!(snapshot[0]["frame"]["event"]["kind"], "snapshot");
        assert_eq!(snapshot[1]["frame"]["event"]["kind"], "state");
        assert_eq!(snapshot[2]["frame"]["event"]["command_id"], "new");
    }

    #[test]
    fn a_later_logged_event_reaches_clients_that_join_after_it() {
        let journal = EventJournal::default();
        journal.publish(serde_json::json!({"kind":"frame", "arm":"left", "frame":{"seq":1, "event":{"kind":"snapshot", "seq":1, "log":[{"seq":1, "event":{"kind":"controller_unavailable"}}]}}}));
        journal.publish(serde_json::json!({"kind":"frame", "arm":"left", "frame":{"seq":2, "event":{"kind":"controller_available"}}}));
        journal.publish(serde_json::json!({"kind":"frame", "arm":"left", "frame":{"seq":null, "event":{"kind":"state"}}}));

        let (_, snapshot, _) = journal.subscribe_with_snapshot();
        let arm_snapshot = &snapshot[0]["frame"]["event"];
        assert_eq!(arm_snapshot["seq"], 2);
        let log = arm_snapshot["log"].as_array().unwrap();
        assert_eq!(log.len(), 2);
        assert_eq!(log[1]["event"]["kind"], "controller_available");
    }

    #[test]
    fn a_later_mode_change_replaces_the_snapshots_mode_and_clears_its_fault() {
        let journal = EventJournal::default();
        journal.publish(serde_json::json!({"kind":"frame", "arm":"left", "frame":{"seq":1, "event":{"kind":"snapshot", "seq":1, "mode":"enabled", "fault":"stale", "log":[]}}}));
        journal.publish(serde_json::json!({"kind":"frame", "arm":"left", "frame":{"seq":2, "event":{"kind":"mode", "mode":"disabled"}}}));

        let (_, snapshot, _) = journal.subscribe_with_snapshot();
        let arm_snapshot = &snapshot[0]["frame"]["event"];
        assert_eq!(arm_snapshot["mode"], "disabled");
        assert!(arm_snapshot["fault"].is_null());
    }

    #[test]
    fn snapshot_preserves_records_for_other_arms_with_shared_name_prefixes() {
        let journal = EventJournal::default();
        journal.publish(serde_json::json!({"kind":"frame", "arm":"left:tool", "frame":{"event":{"kind":"record", "command_id":"move"}}}));
        journal.publish(serde_json::json!({"kind":"frame", "arm":"left", "frame":{"event":{"kind":"record", "command_id":"tool:move"}}}));
        journal.publish(serde_json::json!({"kind":"frame", "arm":"left", "frame":{"event":{"kind":"snapshot"}}}));

        let (_, snapshot, _) = journal.subscribe_with_snapshot();
        assert_eq!(snapshot.len(), 2);
        assert_eq!(snapshot[0]["arm"], "left:tool");
        assert_eq!(snapshot[0]["frame"]["event"]["command_id"], "move");
        assert_eq!(snapshot[1]["arm"], "left");
        assert_eq!(snapshot[1]["frame"]["event"]["kind"], "snapshot");
    }

    #[test]
    fn replay_and_live_delivery_have_no_gap_and_old_cursors_require_resync() {
        let journal = EventJournal::default();
        journal.publish(serde_json::json!({"value": 1}));
        let EventSubscription::Replay {
            events,
            mut updates,
        } = journal.subscribe_after(0)
        else {
            panic!("retained cursor must replay");
        };
        journal.publish(serde_json::json!({"value": 2}));
        assert_eq!(
            events
                .iter()
                .map(|event| event.sequence)
                .collect::<Vec<_>>(),
            [1]
        );
        assert_eq!(updates.try_recv().unwrap().sequence, 2);
        for _ in 0..RETAINED_EVENTS {
            journal.publish(serde_json::json!({"value": 3}));
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

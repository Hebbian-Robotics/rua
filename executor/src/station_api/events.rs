//! Retained station events for resumable client streams.

use std::collections::{BTreeMap, VecDeque};
use std::sync::Mutex;

use tokio::sync::broadcast;

const RETAINED_EVENTS: usize = 4096;

/// One serialized station event and its monotonically increasing cursor.
#[derive(Debug, Clone, PartialEq)]
pub struct StationEvent {
    pub sequence: u64,
    pub payload: serde_json::Value,
}

struct JournalState {
    next_sequence: u64,
    events: VecDeque<StationEvent>,
    snapshots: BTreeMap<String, (u64, serde_json::Value)>,
    records: VecDeque<(String, u64, serde_json::Value)>,
}

/// Events and subscription are captured under one lock to avoid replay gaps.
pub struct EventJournal {
    state: Mutex<JournalState>,
    updates: broadcast::Sender<StationEvent>,
}

/// A cursor outside retained history requires a new snapshot.
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
            let id = format!(
                "{}:{id}",
                payload
                    .get("arm")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
            );
            let sequence = state.next_sequence;
            state.records.retain(|(existing, _, _)| existing != &id);
            state.records.push_back((id, sequence, payload.clone()));
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
                .pointer("/action_sequence/arm")
                .and_then(serde_json::Value::as_str)
                .map(|id| format!("sequence:{id}")),
            _ => None,
        };
        if let Some(key) = snapshot_key {
            if let Some(arm) = key.strip_prefix("snapshot:") {
                let prefix = format!("{arm}:");
                state.records.retain(|(id, _, _)| !id.starts_with(&prefix));
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

    /// Invalidate a stale station snapshot when fresh observations are unavailable.
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

    /// Capture the latest retained state and start live delivery atomically.
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

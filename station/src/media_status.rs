//! Whether the media programs the station supervises are up, as it last saw
//! them: `serve` (camera snapshots) and `publish` (USB cameras into
//! the room). Served at `GET /viewer/api/media-status`; nothing waits on it.

use std::collections::BTreeMap;
use std::sync::{Arc, Condvar, Mutex, PoisonError};

use schemars::JsonSchema;
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum MediaState {
    Running,
    /// Not asked for: `publish` without `LiveKit` credentials or USB cameras.
    NotConfigured,
    /// It exited and will be started again.
    Restarting {
        last_exit: String,
    },
    /// It cannot be started, for instance because the binary is missing.
    Unavailable {
        reason: String,
    },
}

/// The latest state of each supervised program, by name.
#[derive(Debug, Clone)]
pub struct MediaStatus(Arc<StatusInner>);

#[derive(Debug)]
struct StatusInner {
    states: Mutex<BTreeMap<String, MediaState>>,
    changed: Condvar,
}

impl Default for MediaStatus {
    fn default() -> Self {
        let started_without = MediaState::Unavailable {
            reason: "the station was started without a media service".to_owned(),
        };
        Self(Arc::new(StatusInner {
            states: Mutex::new(BTreeMap::from([
                ("publish".to_owned(), MediaState::NotConfigured),
                ("serve".to_owned(), started_without),
            ])),
            changed: Condvar::new(),
        }))
    }
}

impl MediaStatus {
    pub fn set(&self, program: &str, state: MediaState) {
        self.0
            .states
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(program.to_owned(), state);
        self.0.changed.notify_all();
    }

    /// Block until `program`'s state satisfies `wanted`.
    pub fn wait_until(&self, program: &str, wanted: impl Fn(&MediaState) -> bool) {
        let states = self.0.states.lock().unwrap_or_else(PoisonError::into_inner);
        let _states = self
            .0
            .changed
            .wait_while(states, |states| !states.get(program).is_some_and(&wanted))
            .unwrap_or_else(PoisonError::into_inner);
    }

    /// Every program's state, by name.
    #[must_use]
    pub fn all(&self) -> BTreeMap<String, MediaState> {
        self.0
            .states
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    #[must_use]
    pub fn get(&self, program: &str) -> Option<MediaState> {
        self.all().remove(program)
    }
}

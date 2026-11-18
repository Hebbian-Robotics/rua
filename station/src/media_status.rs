use std::collections::BTreeMap;

use std::sync::{Arc, Condvar, Mutex, PoisonError};

use hebbian_contracts::{CameraProblem, MediaState, MediaStatusReport};

#[derive(Debug, Clone)]
pub struct MediaStatus(Arc<StatusInner>);

#[derive(Debug)]
struct StatusInner {
    states: Mutex<BTreeMap<String, MediaState>>,
    camera_problems: Mutex<BTreeMap<String, CameraProblem>>,
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
            camera_problems: Mutex::new(BTreeMap::new()),
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

    pub fn record_camera_problem(&self, camera: &str, detail: &str) {
        let last_seen_unix_seconds = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0.0, |elapsed| elapsed.as_secs_f64());
        self.0
            .camera_problems
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(
                camera.to_owned(),
                CameraProblem {
                    detail: detail.to_owned(),
                    last_seen_unix_seconds,
                },
            );
    }

    #[must_use]
    pub fn report(&self) -> MediaStatusReport {
        MediaStatusReport {
            programs: self.all(),
            cameras: self
                .0
                .camera_problems
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone(),
        }
    }

    pub fn wait_until(&self, program: &str, wanted: impl Fn(&MediaState) -> bool) {
        let states = self.0.states.lock().unwrap_or_else(PoisonError::into_inner);
        let _states = self
            .0
            .changed
            .wait_while(states, |states| !states.get(program).is_some_and(&wanted))
            .unwrap_or_else(PoisonError::into_inner);
    }

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

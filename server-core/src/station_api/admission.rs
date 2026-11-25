use std::sync::Arc;

use hebbian_contracts::{ActionSequenceRecord, ServerMessage};
use tokio::sync::{mpsc, oneshot};

use super::{events::EventJournal, executions::ExecutionRegistry, submissions::Operations};

pub(super) struct AdmissionState {
    pub executions: ExecutionRegistry,
    pub operations: Operations,
    pub clock: fn() -> std::time::Instant,
    events: Arc<EventJournal>,
}

impl AdmissionState {
    pub fn publish_execution(&self, record: ActionSequenceRecord) {
        self.events.publish(ServerMessage::ActionSequenceState {
            action_sequence: Some(record),
        });
    }
}

type Decision = Box<dyn FnOnce(&mut AdmissionState) + Send>;

#[derive(Clone)]
pub(super) struct AdmissionOwner {
    decisions: mpsc::Sender<Decision>,
}

#[derive(Debug)]
pub(super) struct AdmissionClosed;

impl AdmissionOwner {
    pub fn start(runtime: &tokio::runtime::Handle, events: Arc<EventJournal>) -> Self {
        let (decisions, mut pending) = mpsc::channel::<Decision>(64);
        runtime.spawn(async move {
            let mut state = AdmissionState {
                executions: ExecutionRegistry::default(),
                operations: Operations::default(),
                clock: std::time::Instant::now,
                events,
            };
            while let Some(decide) = pending.recv().await {
                decide(&mut state);
            }
        });
        Self { decisions }
    }

    // CONSTRAINT: A decision may enqueue controller work, but must return
    // without waiting for controller feedback, planning, or filesystem I/O.
    pub async fn decide<T: Send + 'static>(
        &self,
        decide: impl FnOnce(&mut AdmissionState) -> T + Send + 'static,
    ) -> Result<T, AdmissionClosed> {
        let (reply, result) = oneshot::channel();
        self.decisions
            .send(Box::new(move |state| {
                let _ = reply.send(decide(state));
            }))
            .await
            .map_err(|_| AdmissionClosed)?;
        result.await.map_err(|_| AdmissionClosed)
    }
}

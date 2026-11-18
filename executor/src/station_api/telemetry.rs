use hebbian_contracts::{LiveRun, RunEvent, ServerMessage, TraceSpan};

#[derive(Debug, Default)]
pub struct RunUpdates {
    pub records: Vec<ServerMessage>,
    pub snapshot: Option<LiveRun>,
}

pub trait RunTelemetry: Send + Sync + 'static {
    fn record_event(&self, event: RunEvent);
    fn record_spans(&self, spans: Vec<TraceSpan>);
    fn newest_seq(&self) -> u64;
    fn updates(&self, cursor: &mut u64, revision_sent: &mut Option<u64>) -> RunUpdates;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct DiscardTelemetry;

impl RunTelemetry for DiscardTelemetry {
    fn record_event(&self, _event: RunEvent) {}
    fn record_spans(&self, _spans: Vec<TraceSpan>) {}
    fn newest_seq(&self) -> u64 {
        0
    }
    fn updates(&self, _cursor: &mut u64, _revision_sent: &mut Option<u64>) -> RunUpdates {
        RunUpdates::default()
    }
}

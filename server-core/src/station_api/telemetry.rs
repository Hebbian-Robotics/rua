use hebbian_contracts::{LiveRun, RunEvent, ServerMessage, TraceSpan};

#[derive(Debug, Default)]
pub struct RunUpdates {
    pub records: Vec<ServerMessage>,
    pub snapshot: Option<LiveRun>,
}

#[derive(Debug, Clone, Copy)]
pub struct RunReplayWindow {
    pub oldest_retained: u64,
    pub newest: u64,
}

pub trait RunTelemetry: Send + Sync + 'static {
    fn record_event(&self, event: RunEvent);
    fn record_spans(&self, spans: Vec<TraceSpan>);
    fn replay_window(&self) -> RunReplayWindow;
    fn updates(&self, cursor: &mut u64, revision_sent: &mut Option<u64>) -> RunUpdates;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct DiscardTelemetry;

impl RunTelemetry for DiscardTelemetry {
    fn record_event(&self, _event: RunEvent) {}
    fn record_spans(&self, _spans: Vec<TraceSpan>) {}
    fn replay_window(&self) -> RunReplayWindow {
        RunReplayWindow {
            oldest_retained: 1,
            newest: 0,
        }
    }
    fn updates(&self, cursor: &mut u64, _revision_sent: &mut Option<u64>) -> RunUpdates {
        let records = if *cursor == 0 {
            Vec::new()
        } else {
            *cursor = 0;
            vec![ServerMessage::RunResync {
                oldest_retained: 1,
                newest: 0,
            }]
        };
        RunUpdates {
            records,
            snapshot: None,
        }
    }
}

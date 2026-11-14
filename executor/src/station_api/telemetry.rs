//! A model loop's live run telemetry as it crosses HTTP: the typed events and
//! trace spans, and the `RunTelemetry` seam through which the process that
//! embeds the executor keeps them. The executor only relays them; the station
//! owns the buffer (`hebbian-station`'s `run_telemetry` module).

use hebbian_contracts::{LiveRun, RunEvent, ServerMessage, TraceSpan};

/// What a connection's run telemetry stream has to send next.
#[derive(Debug, Default)]
pub struct RunUpdates {
    /// Records past the connection's cursor, oldest first, or a `run_resync`.
    pub records: Vec<ServerMessage>,
    /// The live run, when it changed since the connection last got it.
    pub snapshot: Option<LiveRun>,
}

/// Where the station hands a model loop's telemetry and reads it back for viewers.
pub trait RunTelemetry: Send + Sync + 'static {
    fn record_event(&self, event: RunEvent);
    /// Spans as they stand; one with the same ids replaces the earlier one.
    fn record_spans(&self, spans: Vec<TraceSpan>);
    /// The newest sequence number issued; resuming after it misses nothing.
    fn newest_seq(&self) -> u64;
    /// What to send a connection whose last run record was `cursor` and
    /// which last got the run at `revision_sent`; both move forward.
    fn updates(&self, cursor: &mut u64, revision_sent: &mut Option<u64>) -> RunUpdates;
}

/// Telemetry that is dropped, for a server with nothing that keeps run telemetry.
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

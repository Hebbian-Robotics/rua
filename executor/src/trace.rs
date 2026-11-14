//! A record of every control cycle, always on, for measuring the real delay
//! between a command and the arm's motion.
//!
//! One JSON line per cycle in a file named by the run's start time. The loop
//! only pushes into a bounded queue; a writer thread does the file I/O, and a
//! record that finds the queue full is dropped and counted, so the 50 Hz loop
//! never waits on the disk. The field names are camel case, one object per
//! line, so analysis scripts read these files directly.
//!
//! Within a cycle the loop **reads feedback, computes, then writes** the command.
//! A record therefore holds the positions read at `readTime` and the command
//! computed from them and written at `writeTime`; the arm first reacts to that
//! command in the *next* cycle's reading. `seconds`, `readTime` and `writeTime`
//! are monotonic seconds since the run started.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::thread::JoinHandle;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context;
use serde::Serialize;

/// Two seconds of cycles at 500 Hz; far more than the writer ever falls behind.
const QUEUE_LENGTH: usize = 1024;
/// Records written between flushes, so a crash loses little.
const FLUSH_EVERY: u64 = 25;

/// Wall-clock phase durations; queued with the cycle, never logged synchronously.
#[derive(Debug, Default, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
#[allow(clippy::struct_field_names)] // Explicit units in serialized diagnostics.
pub(crate) struct CycleTimings {
    pub requests_seconds: f64,
    pub timer_wait_seconds: f64,
    pub actual_cycle_seconds: f64,
    pub start_next_seconds: f64,
    pub gripper_seconds: f64,
    pub gripper_write_seconds: f64,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TraceRecord {
    pub cycle: u64,
    pub timings: CycleTimings,
    /// When this cycle began.
    pub seconds: f64,
    pub cycle_seconds: f64,
    /// The command written this cycle.
    pub command: Vec<f64>,
    /// What the arm reported at `read_time`, before this cycle's command.
    pub positions: Vec<f64>,
    pub velocities: Vec<f64>,
    pub efforts: Vec<f64>,
    /// Driven motor readings, arm joints first and gripper last; empty if unavailable.
    pub temperatures_celsius: Vec<f64>,
    pub voltages_volts: Vec<f64>,
    pub feedback_fault: Option<String>,
    pub environment_confirmations: Vec<crate::plant::EnvironmentConfirmation>,
    /// When the feedback read returned.
    pub read_time: f64,
    /// When command computation finished and the plant write began.
    pub command_start_time: f64,
    /// When the command write returned.
    pub write_time: f64,
}

/// What a finished trace reports.
#[derive(Debug, Clone)]
pub struct TraceSummary {
    pub path: PathBuf,
    pub written: u64,
    pub dropped: u64,
    /// Set if the writer stopped on an I/O error; later records were dropped.
    pub error: Option<String>,
}

pub(crate) struct TraceWriter {
    sender: Option<SyncSender<TraceRecord>>,
    /// Records the writer has written, sent back so the loop refills their
    /// buffers instead of allocating new ones.
    recycled: Receiver<TraceRecord>,
    dropped: Arc<AtomicU64>,
    writer: Option<JoinHandle<(u64, Option<String>)>>,
    path: PathBuf,
}

impl TraceWriter {
    /// Create the trace file in `directory`, named by the current time.
    ///
    /// # Errors
    /// The directory or file cannot be created: the run must not start without
    /// its trace.
    pub fn open(directory: &Path) -> anyhow::Result<Self> {
        std::fs::create_dir_all(directory).with_context(|| {
            format!("Cannot create the trace directory {}", directory.display())
        })?;
        let nanoseconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .context("The system clock is before 1970")?
            .as_nanos();
        let path = directory.join(format!("executor-{nanoseconds}-trace.jsonl"));
        let file = File::create_new(&path)
            .with_context(|| format!("Cannot create the trace file {}", path.display()))?;
        let (sender, queue) = sync_channel::<TraceRecord>(QUEUE_LENGTH);
        let (recycle, recycled) = sync_channel::<TraceRecord>(QUEUE_LENGTH);
        let writer = std::thread::spawn(move || {
            let mut output = BufWriter::new(file);
            let mut written = 0_u64;
            for record in queue {
                let result = serde_json::to_writer(&mut output, &record)
                    .map_err(std::io::Error::from)
                    .and_then(|()| output.write_all(b"\n"))
                    .and_then(|()| {
                        written += 1;
                        if written.is_multiple_of(FLUSH_EVERY) {
                            output.flush()
                        } else {
                            Ok(())
                        }
                    });
                if let Err(error) = result {
                    return (written, Some(error.to_string()));
                }
                // A full return queue only costs the loop an allocation.
                let _ = recycle.try_send(record);
            }
            (written, output.flush().err().map(|error| error.to_string()))
        });
        Ok(Self {
            sender: Some(sender),
            recycled,
            dropped: Arc::new(AtomicU64::new(0)),
            writer: Some(writer),
            path,
        })
    }

    /// A record to fill: one the writer has finished with, or a new one while
    /// the first records are still in flight.
    pub fn checkout(&self) -> TraceRecord {
        self.recycled.try_recv().unwrap_or_default()
    }

    /// Queue a record; if the writer has fallen behind it is dropped and counted.
    pub fn record(&self, record: TraceRecord) {
        let Some(sender) = &self.sender else { return };
        match sender.try_send(record) {
            Ok(()) => {}
            Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// Write what is queued and report the trace; a writer thread that panicked
    /// is reported as the trace's error.
    #[must_use]
    pub(crate) fn finish(mut self) -> TraceSummary {
        drop(self.sender.take());
        let (written, error) = self.writer.take().map_or((0, None), |writer| {
            writer
                .join()
                .unwrap_or_else(|_| (0, Some("The trace writer panicked".to_owned())))
        });
        TraceSummary {
            path: self.path.clone(),
            written,
            dropped: self.dropped.load(Ordering::Relaxed),
            error,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(cycle: u64) -> TraceRecord {
        TraceRecord {
            cycle,
            timings: CycleTimings::default(),
            seconds: 0.02 * f64::from(u32::try_from(cycle).unwrap()),
            cycle_seconds: 0.02,
            command: vec![0.1],
            positions: vec![0.0],
            velocities: vec![0.0],
            efforts: vec![0.0],
            temperatures_celsius: vec![32.0, 35.0],
            voltages_volts: vec![12.0, 11.8],
            feedback_fault: Some("over temperature".to_owned()),
            environment_confirmations: vec![crate::plant::EnvironmentConfirmation {
                motor_id: 2,
                temperature_celsius: 32.0,
                voltage_volts: 11.9,
                status: 0,
                packet_status: 0,
            }],
            read_time: 0.001,
            command_start_time: 0.0015,
            write_time: 0.002,
        }
    }

    #[test]
    fn every_cycle_becomes_one_line_with_camel_case_field_names() {
        let directory = std::env::temp_dir().join(format!("trace-test-{}", std::process::id()));
        let writer = TraceWriter::open(&directory).unwrap();
        for cycle in 0..60 {
            writer.record(record(cycle));
        }
        let summary = writer.finish();
        assert_eq!((summary.written, summary.dropped), (60, 0));
        let text = std::fs::read_to_string(&summary.path).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 60);
        let first: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        for field in [
            "cycle",
            "seconds",
            "cycleSeconds",
            "command",
            "positions",
            "velocities",
            "efforts",
            "temperaturesCelsius",
            "voltagesVolts",
            "feedbackFault",
            "environmentConfirmations",
            "readTime",
            "writeTime",
        ] {
            assert!(first.get(field).is_some(), "missing {field}");
        }
        assert_eq!(
            first["temperaturesCelsius"],
            serde_json::json!([32.0, 35.0])
        );
        assert_eq!(first["voltagesVolts"], serde_json::json!([12.0, 11.8]));
        assert_eq!(first["feedbackFault"], "over temperature");
        assert_eq!(first["commandStartTime"], 0.0015);
        assert_eq!(first["environmentConfirmations"][0]["motorId"], 2);
        assert_eq!(
            first["environmentConfirmations"][0]["temperatureCelsius"],
            32.0
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn a_writer_that_falls_behind_drops_and_counts_records_instead_of_blocking() {
        let directory = std::env::temp_dir().join(format!("trace-pressure-{}", std::process::id()));
        let writer = TraceWriter::open(&directory).unwrap();
        let offered = QUEUE_LENGTH as u64 * 40;
        for cycle in 0..offered {
            writer.record(record(cycle));
        }
        let summary = writer.finish();
        assert_eq!(summary.written + summary.dropped, offered);
        assert!(summary.error.is_none());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn a_written_record_comes_back_with_its_buffers_to_be_filled_again() {
        let directory = std::env::temp_dir().join(format!("trace-recycle-{}", std::process::id()));
        let writer = TraceWriter::open(&directory).unwrap();
        let mut fresh = writer.checkout();
        fresh.command = Vec::with_capacity(64);
        fresh.command.push(0.1);
        writer.record(fresh);
        let returned = loop {
            let candidate = writer.checkout();
            if candidate.command.capacity() >= 64 {
                break candidate;
            }
            std::thread::yield_now();
        };
        assert!(returned.command.capacity() >= 64);
        let _ = writer.finish();
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn a_trace_that_cannot_be_created_refuses_the_run() {
        let file = std::env::temp_dir().join(format!("trace-blocker-{}", std::process::id()));
        std::fs::write(&file, "not a directory").unwrap();
        assert!(TraceWriter::open(&file.join("inside")).is_err());
        std::fs::remove_file(file).unwrap();
    }
}

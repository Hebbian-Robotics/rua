//! Station telemetry, independent of camera and `LiveKit` connections.
use crate::recorder::{RecorderError, archive::RecordingArchive};
use hebbian_contracts::{Event, Frame, ServerMessage, unix_nanoseconds};
use std::{collections::BTreeMap, time::Duration};
use tokio_util::sync::CancellationToken;

#[derive(serde::Deserialize)]
#[serde(untagged)]
enum IncomingMessage {
    Known(Box<ServerMessage>),
    Unknown(serde_json::Value),
}

pub(crate) async fn subscribe(
    url: &str,
    archive: Option<&RecordingArchive>,
    stopped: CancellationToken,
) -> Result<(), RecorderError> {
    let mut arm_cursors = BTreeMap::new();
    let mut event_cursor = None;
    let mut backoff = hebbian_contracts::Backoff::new(Duration::from_secs(1));
    let mut run_after = Some(0);
    loop {
        let attempt_started = std::time::Instant::now();
        let result = connection(
            url,
            archive,
            &mut arm_cursors,
            &mut run_after,
            &mut event_cursor,
            &stopped,
        )
        .await;
        if stopped.is_cancelled() {
            return Ok(());
        }
        let detail = match result {
            Ok(()) => "station event stream closed".to_owned(),
            Err(error) => error.to_string(),
        };
        eprintln!("Station disconnected; reconnecting with resume: {detail}");
        if let Some(archive) = archive {
            archive.json(
                "/station/recording_errors",
                &serde_json::json!({"source": "station", "detail": detail}),
                unix_nanoseconds(),
            )?;
        }
        tokio::select! { () = stopped.cancelled() => return Ok(()), () = tokio::time::sleep(backoff.next_delay(attempt_started.elapsed())) => {} }
    }
}

async fn connection(
    base_url: &str,
    archive: Option<&RecordingArchive>,
    arm_cursors: &mut BTreeMap<String, u64>,
    run_after: &mut Option<u64>,
    event_cursor: &mut Option<String>,
    stopped: &CancellationToken,
) -> Result<(), RecorderError> {
    let client = reqwest::Client::new();
    let mut base = reqwest::Url::parse(base_url)
        .map_err(|error| RecorderError::InvalidData(error.to_string()))?;
    match base.scheme() {
        "http" | "https" => {}
        _ => return Err(RecorderError::InvalidData("unsupported station URL".into())),
    }
    base.set_path("/events");
    base.set_query(None);
    let mut request = client.get(base.clone());
    if let Some(cursor) = event_cursor.as_deref() {
        request = request.header("Last-Event-ID", cursor);
    }
    let mut response = tokio::select! { () = stopped.cancelled() => return Ok(()), result = request.send() => result? };
    if response.status() == reqwest::StatusCode::CONFLICT && event_cursor.is_some() {
        *event_cursor = None;
        arm_cursors.clear();
        return Err(RecorderError::InvalidData(
            "station event history has a gap".into(),
        ));
    }
    response.error_for_status_ref()?;
    let mut decoder = hebbian_contracts::server_sent_events::ServerSentEventDecoder::default();
    loop {
        let chunk = tokio::select! { () = stopped.cancelled() => return Ok(()), result = response.chunk() => result? };
        let Some(chunk) = chunk else {
            return Ok(());
        };
        decoder.push(&chunk);
        while let Some(event) = decoder
            .next_event()
            .map_err(|error| RecorderError::InvalidData(error.to_string()))?
        {
            apply_event(
                &client,
                &base,
                (&event.kind, &event.data),
                archive,
                arm_cursors,
                run_after,
                event_cursor,
            )
            .await?;
            if let Some(identifier) = event.identifier.filter(|identifier| !identifier.is_empty()) {
                *event_cursor = Some(identifier);
            }
        }
    }
}

async fn apply_event(
    client: &reqwest::Client,
    base: &reqwest::Url,
    event: (&str, &str),
    archive: Option<&RecordingArchive>,
    arm_cursors: &mut BTreeMap<String, u64>,
    run_after: &mut Option<u64>,
    event_cursor: &mut Option<String>,
) -> Result<(), RecorderError> {
    let (kind, data) = event;
    if kind == "resync_required" {
        *event_cursor = None;
        arm_cursors.clear();
        return Err(RecorderError::InvalidData(
            "station event history has a gap".into(),
        ));
    }
    let payload: serde_json::Value = serde_json::from_str(data)?;
    if payload.get("kind").and_then(serde_json::Value::as_str) == Some("resync_required") {
        *event_cursor = None;
        arm_cursors.clear();
        return Err(RecorderError::InvalidData(
            "station event history has a gap".into(),
        ));
    }
    if kind == "snapshot" {
        if let Some(after) = *run_after {
            let mut url = base.clone();
            url.set_path("/run/records");
            let records: Vec<serde_json::Value> = client
                .get(url)
                .query(&[("after", after)])
                .timeout(Duration::from_secs(10))
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            for record in records {
                record_message(record, archive, arm_cursors, run_after)?;
            }
        }
        let items = payload
            .get("items")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| RecorderError::InvalidData("invalid station snapshot".into()))?;
        for item in items {
            record_message(item.clone(), archive, arm_cursors, run_after)?;
        }
    } else if kind == "station" {
        record_message(payload, archive, arm_cursors, run_after)?;
    }
    Ok(())
}

fn record_message(
    payload: serde_json::Value,
    archive: Option<&RecordingArchive>,
    resume_after: &mut BTreeMap<String, u64>,
    run_after: &mut Option<u64>,
) -> Result<(), RecorderError> {
    let message = match serde_json::from_value::<IncomingMessage>(payload)? {
        IncomingMessage::Known(message) => *message,
        IncomingMessage::Unknown(raw) => {
            if let Some(archive) = archive {
                archive.json("/station/unknown_messages", &raw, unix_nanoseconds())?;
            }
            return Ok(());
        }
    };
    match message {
        ServerMessage::State { state } => {
            if let Some(archive) = archive {
                record_state(archive, &state)?;
            }
        }
        ServerMessage::Frame { arm, frame } => record_frame(&arm, &frame, archive, resume_after)?,
        ServerMessage::Run { run } => {
            if let Some(archive) = archive {
                archive.json(
                    "/run/snapshot",
                    &serde_json::to_value(&run)?,
                    unix_nanoseconds(),
                )?;
            }
        }
        ServerMessage::RunEvent {
            seq,
            received_unix_nano,
            event,
        } => {
            if run_after.is_none_or(|previous| seq > previous) {
                if let Some(archive) = archive {
                    archive.json("/model/events", &serde_json::json!({"seq": seq, "received_unix_nano": received_unix_nano, "event": event}), received_unix_nano.0)?;
                }
                *run_after = Some(seq);
            }
        }
        ServerMessage::RunSpan {
            seq,
            received_unix_nano,
            span,
        } => {
            if run_after.is_none_or(|previous| seq > previous) {
                if let Some(archive) = archive {
                    let mut output = serde_json::to_value(recorded_span(&span)?)?;
                    output["received_unix_nano"] = serde_json::to_value(received_unix_nano)?;
                    output["seq"] = seq.into();
                    archive.json("/trace/spans", &output, received_unix_nano.0)?;
                }
                *run_after = Some(seq);
            }
        }
        ServerMessage::RunResync {
            oldest_retained,
            newest,
        } => {
            if let Some(archive) = archive {
                archive.json("/station/recording_errors", &serde_json::json!({"source": "run", "detail": "Run telemetry resume gap", "after": run_after, "oldest_retained": oldest_retained, "newest": newest}), unix_nanoseconds())?;
            }
            *run_after = Some(newest);
        }
        _ => {}
    }
    Ok(())
}

fn record_frame(
    arm: &str,
    frame: &Frame,
    archive: Option<&RecordingArchive>,
    resume_after: &mut BTreeMap<String, u64>,
) -> Result<(), RecorderError> {
    if frame.seq.is_some_and(|sequence| {
        resume_after
            .get(arm)
            .is_some_and(|previous| sequence <= *previous)
    }) {
        return Ok(());
    }
    if let Some(archive) = archive {
        match &frame.event {
            Event::Record(record) => record_command(arm, record, archive, frame.emitted_unix_nano.0)?,
            Event::State(sample) => archive.json(
                &format!("/station/control/{arm}"),
                &serde_json::to_value(sample)?,
                frame.emitted_unix_nano.0,
            )?,
            Event::Snapshot {
                seq, records, log, ..
            } => {
                for retained_frame in log {
                    record_frame(arm, retained_frame, Some(archive), resume_after)?;
                }
                for record in records {
                    record_command(arm, record, archive, frame.emitted_unix_nano.0)?;
                }
                if log.iter().rev().find_map(|frame| match frame.event {
                    Event::ControllerUnavailable { .. } => Some(false),
                    Event::ControllerAvailable => Some(true),
                    _ => None,
                }) == Some(false) {
                    archive.mark_controller_interrupted(frame.emitted_unix_nano.0, true)?;
                }
                resume_after.insert(arm.into(), *seq);
            }
            Event::Resync { after, oldest_retained, newest } => archive.json(
                "/station/recording_errors",
                &serde_json::json!({"source": "station", "arm": arm, "detail": "Arm resume gap", "after": after, "oldest_retained": oldest_retained, "newest": newest}),
                frame.emitted_unix_nano.0,
            )?,
            Event::Rejected { reason } | Event::Tripped { reason } => archive.json(
                "/station/rejections",
                &serde_json::json!({"arm": arm, "detail": reason}),
                frame.emitted_unix_nano.0,
            )?,
            Event::ControllerUnavailable { reason } => {
                archive.mark_controller_interrupted(frame.emitted_unix_nano.0, false)?;
                archive.json("/station/controller", &serde_json::json!({"arm": arm, "state": "unavailable", "reason": reason}), frame.emitted_unix_nano.0)?;
            }
            Event::ControllerAvailable => archive.json("/station/controller", &serde_json::json!({"arm": arm, "state": "available"}), frame.emitted_unix_nano.0)?,
            _ => {}
        }
    }
    let sequence = match &frame.event {
        Event::Snapshot { seq, .. } => Some(*seq),
        _ => frame.seq,
    };
    if let Some(sequence) = sequence {
        resume_after.insert(arm.into(), sequence);
    }
    Ok(())
}

// CONSTRAINT: MCAP stores spans in the viewer's OpenTelemetry JSON format.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct RecordedSpan {
    trace_id: String,
    span_id: String,
    parent_span_id: Option<String>,
    name: String,
    start_time_unix_nano: String,
    end_time_unix_nano: String,
    attributes: BTreeMap<String, serde_json::Value>,
    status: RecordedSpanStatus,
}

#[derive(serde::Serialize)]
struct RecordedSpanStatus {
    code: String,
    message: Option<String>,
}

fn recorded_span(span: &hebbian_contracts::TraceSpan) -> Result<RecordedSpan, RecorderError> {
    Ok(RecordedSpan {
        trace_id: span.trace_id.clone(),
        span_id: span.span_id.clone(),
        parent_span_id: span.parent_span_id.clone(),
        name: span.name.clone(),
        start_time_unix_nano: span.start_time_unix_nano.0.to_string(),
        end_time_unix_nano: span.end_time_unix_nano.0.to_string(),
        attributes: span.attributes.clone(),
        status: RecordedSpanStatus {
            code: serde_json::from_value(serde_json::to_value(span.status.code)?)?,
            message: span.status.message.clone(),
        },
    })
}

fn record_command(
    arm: &str,
    record: &hebbian_contracts::CommandRecord,
    archive: &RecordingArchive,
    timestamp: u64,
) -> Result<(), RecorderError> {
    archive.json(
        "/station/command_records",
        &serde_json::to_value(hebbian_contracts::ArmRecord {
            arm: arm.into(),
            record: record.clone(),
        })?,
        timestamp,
    )?;
    archive.json(
        "/trace/spans",
        &serde_json::to_value(command_span(arm, record))?,
        timestamp,
    )
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // CONSTRAINT: station command times are Unix seconds; span timestamps are nanoseconds.
fn command_span(arm: &str, record: &hebbian_contracts::CommandRecord) -> RecordedSpan {
    use sha2::{Digest, Sha256};
    let digest = format!(
        "{:x}",
        Sha256::digest(format!("{arm}\0{}", record.command_id))
    );
    let parent = record.trace_context.as_ref().and_then(|context| {
        let parts: Vec<_> = context.traceparent.split('-').collect();
        (parts.len() == 4
            && parts[0] == "00"
            && parts[3].len() == 2
            && parts[3]
                .bytes()
                .all(|character| character.is_ascii_hexdigit())
            && parts[1].len() == 32
            && parts[2].len() == 16
            && parts[1]
                .bytes()
                .chain(parts[2].bytes())
                .all(|character| character.is_ascii_hexdigit())
            && parts[1] != "00000000000000000000000000000000"
            && parts[2] != "0000000000000000")
            .then(|| (parts[1].to_owned(), parts[2].to_owned()))
    });
    if record.trace_context.is_some() && parent.is_none() {
        eprintln!(
            "Command {} has an invalid traceparent; retaining its context and using a root span",
            record.command_id
        );
    }
    RecordedSpan {
        trace_id: parent
            .as_ref()
            .map_or_else(|| digest[..32].into(), |(trace, _)| trace.clone()),
        span_id: digest[32..48].into(),
        parent_span_id: parent.map(|(_, span)| span),
        name: format!("{arm}: {}", record.label),
        start_time_unix_nano: ((record.accepted_at * 1e9) as u64).to_string(),
        end_time_unix_nano: record
            .finished_at()
            .map_or_else(|| "0".to_owned(), |time| ((time * 1e9) as u64).to_string()),
        attributes: BTreeMap::from([
            ("hebbian.active".into(), record.completion.is_none().into()),
            ("hebbian.kind".into(), "command".into()),
            ("arm".into(), arm.into()),
            ("command_id".into(), record.command_id.clone().into()),
            ("outcome".into(), serde_json::json!(record.outcome())),
            (
                "trace_context".into(),
                serde_json::json!(record.trace_context),
            ),
        ]),
        status: RecordedSpanStatus {
            code: match record.outcome() {
                None => "STATUS_CODE_UNSET",
                Some(
                    hebbian_contracts::CommandOutcome::Completed
                    | hebbian_contracts::CommandOutcome::Stopped
                    | hebbian_contracts::CommandOutcome::Superseded,
                ) => "STATUS_CODE_OK",
                Some(
                    hebbian_contracts::CommandOutcome::StoppedShort
                    | hebbian_contracts::CommandOutcome::Tripped
                    | hebbian_contracts::CommandOutcome::TimedOut
                    | hebbian_contracts::CommandOutcome::Faulted
                    | hebbian_contracts::CommandOutcome::Interrupted
                    | hebbian_contracts::CommandOutcome::Failed,
                ) => "STATUS_CODE_ERROR",
            }
            .into(),
            message: record.detail().map(str::to_owned),
        },
    }
}

/// A plant that reports no velocity or effort leaves that array out of the message.
fn one_per_joint(values: &[f64], joint_count: usize) -> &[f64] {
    if values.len() == joint_count {
        values
    } else {
        &[]
    }
}

/// The station's state as JSON, and each arm's joints as a ROS 2 `JointState`.
fn record_state(
    archive: &RecordingArchive,
    state: &hebbian_contracts::StationState,
) -> Result<(), RecorderError> {
    let now = unix_nanoseconds();
    archive.json("/station/state", &serde_json::to_value(state)?, now)?;
    for arm in &state.arms {
        if arm.sampled_at_unix_seconds.is_some()
            && arm.measured_joints.len() == state.joint_names.len()
        {
            archive.joint_states(
                &arm.arm,
                now,
                &state.joint_names,
                &arm.measured_joints,
                one_per_joint(&arm.measured_joint_velocities, state.joint_names.len()),
                one_per_joint(&arm.measured_joint_efforts, state.joint_names.len()),
            )?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interrupted_commands_are_recorded_as_errors() -> Result<(), Box<dyn std::error::Error>> {
        let record: hebbian_contracts::CommandRecord = serde_json::from_value(serde_json::json!({
            "command_id": "command-1", "label": "move", "accepted_at": 1.0,
            "completion": {"finished_at": 2.0, "outcome": "interrupted", "detail": "Disconnected"},
            "checkpoints": []
        }))?;
        let span = command_span("follower", &record);
        assert_eq!(span.status.code, "STATUS_CODE_ERROR");
        assert_eq!(span.status.message.as_deref(), Some("Disconnected"));
        Ok(())
    }

    #[test]
    fn command_revisions_and_arm_gaps_keep_trace_and_emission_times()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let archive = RecordingArchive::create(directory.path())?;
        let mut cursors = BTreeMap::new();
        let mut record: hebbian_contracts::CommandRecord = serde_json::from_value(
            serde_json::json!({
                "command_id": "command-1", "label": "move", "accepted_at": 1.25,
                "checkpoints": [], "trace_context": {"traceparent": "00-11111111111111111111111111111111-2222222222222222-01"}
            }),
        )?;
        for (sequence, timestamp) in [(1, 1_300_000_000), (2, 2_600_000_000)] {
            if sequence == 2 {
                record.completion = Some(hebbian_contracts::CommandCompletion {
                    finished_at: 2.5,
                    outcome: hebbian_contracts::CommandOutcome::Completed,
                    detail: None,
                    gripper: None,
                });
            }
            record_frame(
                "left",
                &Frame {
                    seq: Some(sequence),
                    emitted_unix_nano: hebbian_contracts::UnixNanos(timestamp),
                    event: Event::Record(record.clone()),
                },
                Some(&archive),
                &mut cursors,
            )?;
        }
        record_frame(
            "left",
            &Frame {
                seq: None,
                emitted_unix_nano: hebbian_contracts::UnixNanos(3_000_000_000),
                event: Event::Resync {
                    after: 2,
                    oldest_retained: 8,
                    newest: 10,
                },
            },
            Some(&archive),
            &mut cursors,
        )?;
        archive.finish()?;
        let data = std::fs::read(directory.path().join("recording.mcap"))?;
        let messages = mcap::MessageStream::new(&data)?.collect::<Result<Vec<_>, _>>()?;
        let spans = messages
            .iter()
            .filter(|message| message.channel.topic == "/trace/spans")
            .map(|message| serde_json::from_slice::<serde_json::Value>(&message.data))
            .collect::<Result<Vec<_>, _>>()?;
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0]["spanId"], spans[1]["spanId"]);
        assert_eq!(spans[1]["traceId"], "11111111111111111111111111111111");
        assert_eq!(spans[1]["parentSpanId"], "2222222222222222");
        assert_eq!(spans[1]["startTimeUnixNano"], "1250000000");
        assert_eq!(spans[1]["endTimeUnixNano"], "2500000000");
        assert_eq!(spans[1]["attributes"]["outcome"], "completed");
        assert_eq!(messages[0].log_time, 1_300_000_000);
        let gap = messages.last().ok_or("missing gap")?;
        assert_eq!(gap.channel.topic, "/station/recording_errors");
        assert_eq!(gap.log_time, 3_000_000_000);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&gap.data)?["after"],
            2
        );
        Ok(())
    }
}

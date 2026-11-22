//! Station telemetry, independent of camera and `LiveKit` connections.
use crate::recorder::{RecorderError, archive::RecordingArchive};
use hebbian_contracts::{Backoff, Event, Frame, ServerMessage, unix_nanoseconds};
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
    let mut event_cursor = None;
    let mut backoff = Backoff::new(Duration::from_secs(1));
    loop {
        let result = connection(url, archive, &mut event_cursor, &mut backoff, &stopped).await;
        if stopped.is_cancelled() {
            return Ok(());
        }
        let detail = match result {
            Ok(()) => "station event stream closed".to_owned(),
            Err(error) => error.to_string(),
        };
        tracing::warn!(%detail, "Station disconnected; reconnecting with resume");
        if let Some(archive) = archive {
            archive.json(
                "/station/recording_errors",
                &serde_json::json!({"source": "station", "detail": detail}),
                unix_nanoseconds(),
            )?;
        }
        tokio::select! { () = stopped.cancelled() => return Ok(()), () = tokio::time::sleep(backoff.next_delay()) => {} }
    }
}

async fn connection(
    base_url: &str,
    archive: Option<&RecordingArchive>,
    event_cursor: &mut Option<String>,
    backoff: &mut Backoff,
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
    let mut response = loop {
        let mut request = client.get(base.clone());
        if let Some(cursor) = event_cursor.as_deref() {
            request = request.header("Last-Event-ID", cursor);
        }
        let response = tokio::select! { () = stopped.cancelled() => return Ok(()), result = request.send() => result? };
        if response.status() == reqwest::StatusCode::CONFLICT
            && let Some(after) = event_cursor.take()
        {
            if let Some(archive) = archive {
                record_gap(
                    archive,
                    "Station event history has a gap",
                    Some(&after),
                    None,
                )?;
            }
        } else {
            break response.error_for_status()?;
        }
    };
    let mut applied_station_event = false;
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
            if event.kind == "resync_required" {
                // WHY: The last successfully archived event still determines
                // whether the server can replay the stream or must report a gap.
                return Ok(());
            }
            apply_event(&event.kind, &event.data, archive)?;
            if !applied_station_event && matches!(event.kind.as_str(), "snapshot" | "station") {
                applied_station_event = true;
                backoff.reset();
            }
            if let Some(identifier) = event.identifier.filter(|identifier| !identifier.is_empty()) {
                *event_cursor = Some(identifier);
            }
        }
    }
}

fn apply_event(
    kind: &str,
    data: &str,
    archive: Option<&RecordingArchive>,
) -> Result<(), RecorderError> {
    let Some(archive) = archive else {
        return Ok(());
    };
    let payload: serde_json::Value = serde_json::from_str(data)?;
    match kind {
        "snapshot" => {
            let items = payload
                .get("items")
                .and_then(serde_json::Value::as_array)
                .ok_or_else(|| RecorderError::InvalidData("invalid station snapshot".into()))?;
            for item in items {
                record_message(item.clone(), archive, Delivery::Snapshot)?;
            }
        }
        "station" => {
            if payload.get("kind").and_then(serde_json::Value::as_str) == Some("resync_required") {
                record_gap(
                    archive,
                    payload
                        .get("detail")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("Station source history has a gap"),
                    None,
                    payload.get("arm").and_then(serde_json::Value::as_str),
                )?;
            } else {
                record_message(payload, archive, Delivery::Event)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn record_gap(
    archive: &RecordingArchive,
    detail: &str,
    after: Option<&str>,
    arm: Option<&str>,
) -> Result<(), RecorderError> {
    archive.json(
        "/station/recording_errors",
        &serde_json::json!({"source": "station", "detail": detail, "after": after, "arm": arm}),
        unix_nanoseconds(),
    )
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Delivery {
    Snapshot,
    Event,
}

fn record_message(
    payload: serde_json::Value,
    archive: &RecordingArchive,
    delivery: Delivery,
) -> Result<(), RecorderError> {
    let message = match serde_json::from_value::<IncomingMessage>(payload)? {
        IncomingMessage::Known(message) => *message,
        IncomingMessage::Unknown(raw) => {
            archive.json("/station/unknown_messages", &raw, unix_nanoseconds())?;
            return Ok(());
        }
    };
    match message {
        ServerMessage::State { state } => record_state(archive, &state)?,
        ServerMessage::Frame { arm, frame } => record_frame(&arm, &frame, archive, delivery)?,
        ServerMessage::Run { run } => archive.json(
            "/run/snapshot",
            &serde_json::to_value(&run)?,
            unix_nanoseconds(),
        )?,
        ServerMessage::RunEvent {
            seq,
            received_unix_nano,
            event,
        } => {
            archive.json("/model/events", &serde_json::json!({"seq": seq, "received_unix_nano": received_unix_nano, "event": event}), received_unix_nano.0)?;
        }
        ServerMessage::RunSpan {
            seq,
            received_unix_nano,
            span,
        } => {
            let mut output = serde_json::to_value(recorded_span(&span)?)?;
            output["received_unix_nano"] = serde_json::to_value(received_unix_nano)?;
            output["seq"] = seq.into();
            archive.json("/trace/spans", &output, received_unix_nano.0)?;
        }
        ServerMessage::RunResync {
            oldest_retained,
            newest,
        } => {
            archive.json("/station/recording_errors", &serde_json::json!({"source": "run", "detail": "Run telemetry resume gap", "oldest_retained": oldest_retained, "newest": newest}), unix_nanoseconds())?;
        }
        _ => {}
    }
    Ok(())
}

fn record_frame(
    arm: &str,
    frame: &Frame,
    archive: &RecordingArchive,
    delivery: Delivery,
) -> Result<(), RecorderError> {
    let emitted_at = frame.emitted_unix_nano.0;
    match &frame.event {
        Event::Record(record) if delivery == Delivery::Event => record_command(arm, record, archive, emitted_at)?,
        Event::State(sample) => archive.json(
            &format!("/station/control/{arm}"),
            &serde_json::to_value(sample)?,
            emitted_at,
        )?,
        Event::Snapshot { log, .. } => {
            // WHY: A snapshot is a baseline. Its embedded log overlaps the journal.
            if let Some(status) = log.iter().rev().find(|entry| matches!(
                entry.event, Event::ControllerAvailable | Event::ControllerUnavailable { .. }
            )) {
                record_controller(arm, &status.event, archive, emitted_at, true)?;
            }
        }
        Event::Resync { after, oldest_retained, newest } => archive.json(
            "/station/recording_errors",
            &serde_json::json!({"source": "station", "arm": arm, "detail": "Arm resume gap", "after": after, "oldest_retained": oldest_retained, "newest": newest}),
            emitted_at,
        )?,
        Event::Rejected { reason } | Event::Tripped { reason } => archive.json(
            "/station/rejections",
            &serde_json::json!({"arm": arm, "detail": reason}),
            emitted_at,
        )?,
        Event::ControllerAvailable | Event::ControllerUnavailable { .. } => {
            record_controller(arm, &frame.event, archive, emitted_at, false)?;
        }
        _ => {}
    }
    Ok(())
}

fn record_controller(
    arm: &str,
    event: &Event,
    archive: &RecordingArchive,
    emitted_at: u64,
    baseline: bool,
) -> Result<(), RecorderError> {
    let state = match event {
        Event::ControllerUnavailable { reason } => {
            archive.mark_controller_interrupted(emitted_at, baseline)?;
            serde_json::json!({"arm": arm, "state": "unavailable", "reason": reason})
        }
        Event::ControllerAvailable => serde_json::json!({"arm": arm, "state": "available"}),
        _ => return Ok(()),
    };
    archive.json("/station/controller", &state, emitted_at)
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
        tracing::warn!(
            command_id = %record.command_id,
            "Invalid traceparent; retaining its context and using a root span"
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
            ("hebbian.active".into(), record.is_pending().into()),
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
            "command": {"kind": "joint_stream", "goal_joints": [0.1], "joint_speed": 1.0},
            "phase": {"kind": "outcome_unknown", "finished_at": 2.0, "detail": "Disconnected"},
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
        let mut record: hebbian_contracts::CommandRecord = serde_json::from_value(
            serde_json::json!({
                "command_id": "command-1", "label": "move", "accepted_at": 1.25,
                "phase": {"kind": "pending"},
                "command": {"kind": "joint_stream", "goal_joints": [0.1], "joint_speed": 1.0},
                "checkpoints": [], "trace_context": {"traceparent": "00-11111111111111111111111111111111-2222222222222222-01"}
            }),
        )?;
        for (sequence, timestamp) in [(1, 1_300_000_000), (2, 2_600_000_000)] {
            if sequence == 2 {
                record.phase = hebbian_contracts::CommandPhase::Finished {
                    completion: hebbian_contracts::CommandCompletion {
                        finished_at: 2.5,
                        outcome: hebbian_contracts::KnownCommandOutcome::Completed,
                        detail: None,
                        gripper: None,
                    },
                    achieved: None,
                };
            }
            record_frame(
                "left",
                &Frame {
                    seq: Some(sequence),
                    emitted_unix_nano: hebbian_contracts::UnixNanos(timestamp),
                    event: Event::Record(record.clone()),
                },
                &archive,
                Delivery::Event,
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
            &archive,
            Delivery::Event,
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

use super::{RecorderError, archive::RecordingArchive};
use hebbian_contracts::server_sent_events::ServerSentEvent;
use hebbian_contracts::{
    Backoff, Event, Frame, ServerMessage, StationEventSnapshot, unix_nanoseconds,
};
use std::{collections::BTreeMap, time::Duration};
use tokio_util::sync::CancellationToken;

#[derive(serde::Deserialize)]
#[serde(untagged)]
enum IncomingMessage {
    Known(Box<ServerMessage>),
    SourceGap(SourceGap),
    Unknown(serde_json::Value),
}

#[derive(serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum SourceGap {
    ResyncRequired {
        detail: Option<String>,
        arm: Option<String>,
    },
}

enum RecorderDelivery {
    Reconnect,
    Apply {
        delivery: Option<Delivery>,
        data: String,
        identifier: Option<String>,
    },
}

impl From<ServerSentEvent> for RecorderDelivery {
    fn from(event: ServerSentEvent) -> Self {
        let delivery = match event.kind.as_str() {
            "snapshot" => Some(Delivery::Snapshot),
            "station" => Some(Delivery::Event),
            "resync_required" => return Self::Reconnect,
            _ => None,
        };
        Self::Apply {
            delivery,
            data: event.data,
            identifier: event.identifier,
        }
    }
}

pub(super) async fn subscribe(
    url: &str,
    archive: Option<&RecordingArchive>,
    stopped: CancellationToken,
) -> Result<(), RecorderError> {
    let mut base =
        reqwest::Url::parse(url).map_err(|error| RecorderError::InvalidData(error.to_string()))?;
    match base.scheme() {
        "http" | "https" => {}
        _ => return Err(RecorderError::InvalidData("unsupported station URL".into())),
    }
    base.set_path("/events");
    base.set_query(None);
    let client = reqwest::Client::builder()
        .read_timeout(Duration::from_secs(30))
        .build()?;
    subscribe_to_events(&client, &base, archive, stopped).await
}

async fn subscribe_to_events(
    client: &reqwest::Client,
    events_url: &reqwest::Url,
    archive: Option<&RecordingArchive>,
    stopped: CancellationToken,
) -> Result<(), RecorderError> {
    let mut event_cursor = None;
    let mut backoff = Backoff::new(Duration::from_secs(1));
    loop {
        let result = connection(
            client,
            events_url,
            archive,
            &mut event_cursor,
            &mut backoff,
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
    client: &reqwest::Client,
    events_url: &reqwest::Url,
    archive: Option<&RecordingArchive>,
    event_cursor: &mut Option<String>,
    backoff: &mut Backoff,
    stopped: &CancellationToken,
) -> Result<(), RecorderError> {
    let mut response = loop {
        let mut request = client.get(events_url.clone());
        if let Some(cursor) = event_cursor.as_deref() {
            request = request.header("Last-Event-ID", cursor);
        }
        let response = tokio::select! {
            () = stopped.cancelled() => return Ok(()),
            result = tokio::time::timeout(Duration::from_secs(5), request.send()) => {
                result.map_err(|_| RecorderError::Timeout("station event stream handshake"))??
            }
        };
        if response.status() == reqwest::StatusCode::CONFLICT
            && let Some(after) = event_cursor.as_deref()
        {
            if let Some(archive) = archive {
                record_gap(
                    archive,
                    "Station event history has a gap",
                    Some(after),
                    None,
                )?;
            }
            *event_cursor = None;
        } else {
            break response.error_for_status()?;
        }
    };

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
            let RecorderDelivery::Apply {
                delivery,
                data,
                identifier,
            } = RecorderDelivery::from(event)
            else {
                return Ok(());
            };
            if let Some(delivery) = delivery {
                if let Some(archive) = archive {
                    delivery.record(&data, archive)?;
                }
                backoff.reset();
            }
            // INVARIANT: Only a successfully applied delivery can change the replay position.
            if let Some(identifier) = identifier {
                *event_cursor = (!identifier.is_empty()).then_some(identifier);
            }
        }
    }
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

impl Delivery {
    fn record(self, data: &str, archive: &RecordingArchive) -> Result<(), RecorderError> {
        match self {
            Self::Snapshot => {
                let snapshot: StationEventSnapshot<IncomingMessage> = serde_json::from_str(data)?;
                for message in snapshot.items {
                    record_message(message, archive, self)?;
                }
                Ok(())
            }
            Self::Event => record_message(serde_json::from_str(data)?, archive, self),
        }
    }
}

fn record_message(
    incoming: IncomingMessage,
    archive: &RecordingArchive,
    delivery: Delivery,
) -> Result<(), RecorderError> {
    let message = match incoming {
        IncomingMessage::SourceGap(SourceGap::ResyncRequired { detail, arm }) => {
            return record_gap(
                archive,
                detail
                    .as_deref()
                    .unwrap_or("Station source history has a gap"),
                None,
                arm.as_deref(),
            );
        }
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
        start_time_unix_nano: record.accepted_unix_nano.0.to_string(),
        end_time_unix_nano: record
            .finished_unix_nano()
            .map_or_else(|| "0".to_owned(), |time| time.0.to_string()),
        attributes: BTreeMap::from([
            ("hebbian.active".into(), record.is_pending().into()),
            ("hebbian.kind".into(), "command".into()),
            ("arm".into(), arm.into()),
            ("command_id".into(), record.command_id.to_string().into()),
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

// CONSTRAINT: A plant that reports no velocity or effort leaves that array out of the message.
fn one_per_joint(values: &[f64], joint_count: usize) -> &[f64] {
    if values.len() == joint_count {
        values
    } else {
        &[]
    }
}

fn record_state(
    archive: &RecordingArchive,
    state: &hebbian_contracts::StationState,
) -> Result<(), RecorderError> {
    let now = unix_nanoseconds();
    archive.json("/station/state", &serde_json::to_value(state)?, now)?;
    for arm in &state.arms {
        if arm.sampled_unix_nano.is_some() && arm.measured_joints.len() == state.joint_names.len() {
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

    #[tokio::test]
    async fn a_silent_event_stream_reconnects_with_its_last_cursor()
    -> Result<(), Box<dyn std::error::Error>> {
        use axum::{
            Router,
            http::HeaderMap,
            response::sse::{Event as SseEvent, Sse},
            routing::get,
        };

        let (cursors, mut requests) = tokio::sync::mpsc::unbounded_channel();
        let router = Router::new().route(
            "/events",
            get(move |headers: HeaderMap| {
                let cursors = cursors.clone();
                async move {
                    let cursor = headers
                        .get("last-event-id")
                        .and_then(|header| header.to_str().ok())
                        .map(str::to_owned);
                    let _ = cursors.send(cursor);
                    let mut event = Some(
                        SseEvent::default()
                            .id("event-42")
                            .event("station")
                            .data("{}"),
                    );
                    Sse::new(futures_util::stream::poll_fn(move |_| match event.take() {
                        Some(event) => {
                            std::task::Poll::Ready(Some(Ok::<_, std::convert::Infallible>(event)))
                        }
                        None => std::task::Poll::Pending,
                    }))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let events_url = reqwest::Url::parse(&format!("http://{}/events", listener.local_addr()?))?;
        let server = tokio::spawn(async move { axum::serve(listener, router).await });
        let stopped = CancellationToken::new();
        let client = reqwest::Client::builder()
            .read_timeout(Duration::from_millis(300))
            .build()?;
        let client_stopped = stopped.clone();
        let subscriber = tokio::spawn(async move {
            subscribe_to_events(&client, &events_url, None, client_stopped).await
        });
        let observed = tokio::time::timeout(Duration::from_secs(5), async {
            (requests.recv().await, requests.recv().await)
        })
        .await;
        stopped.cancel();
        subscriber.await??;
        server.abort();
        assert_eq!(observed?, (Some(None), Some(Some("event-42".to_owned()))));
        Ok(())
    }

    #[tokio::test]
    async fn replay_resumes_after_the_last_applied_delivery()
    -> Result<(), Box<dyn std::error::Error>> {
        use axum::{Router, routing::get};
        let snapshot = "id: applied\nevent: snapshot\ndata: {\"station_instance_id\":\"station\",\"sequence\":1,\"run_sequence\":0,\"items\":[]}\n\n";
        for (next_event, expected_cursor, success) in [
            (
                "id: gap\nevent: resync_required\ndata: lagged\n\n",
                Some("applied"),
                true,
            ),
            (
                "id: broken\nevent: snapshot\ndata: {}\n\n",
                Some("applied"),
                false,
            ),
            (
                "id:\nevent: station\ndata: {\"kind\":\"hello\",\"arms\":[]}\n\n",
                None,
                true,
            ),
        ] {
            let body = format!("{snapshot}{next_event}");
            let router = Router::new().route("/events", get(move || async move { body }));
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
            let url = reqwest::Url::parse(&format!("http://{}/events", listener.local_addr()?))?;
            let server = tokio::spawn(async move { axum::serve(listener, router).await });
            let directory = tempfile::tempdir()?;
            let archive = RecordingArchive::create(directory.path())?;
            let mut cursor = None;
            let result = connection(
                &reqwest::Client::new(),
                &url,
                Some(&archive),
                &mut cursor,
                &mut Backoff::new(Duration::from_secs(1)),
                &CancellationToken::new(),
            )
            .await;
            server.abort();
            assert_eq!(
                (cursor.as_deref(), result.is_ok()),
                (expected_cursor, success)
            );
        }
        Ok(())
    }

    #[test]
    fn interrupted_commands_are_recorded_as_errors() -> Result<(), Box<dyn std::error::Error>> {
        let record: hebbian_contracts::CommandRecord = serde_json::from_value(serde_json::json!({
            "command_id": "command-1", "label": "move", "accepted_unix_nano": "1700000000000000238",
            "command": {"kind": "joint_stream", "goal_joints": [0.1], "joint_speed": 1.0},
            "phase": {"kind": "outcome_unknown", "finished_unix_nano": "2000000000", "detail": "Disconnected"},
            "checkpoints": []
        }))?;
        let span = command_span("follower", &record);
        assert_eq!(span.start_time_unix_nano, "1700000000000000238");
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
                "command_id": "command-1", "label": "move", "accepted_unix_nano": "1250000000",
                "phase": {"kind": "pending"},
                "command": {"kind": "joint_stream", "goal_joints": [0.1], "joint_speed": 1.0},
                "checkpoints": [], "trace_context": {"traceparent": "00-11111111111111111111111111111111-2222222222222222-01"}
            }),
        )?;
        for (sequence, timestamp) in [(1, 1_300_000_000), (2, 2_600_000_000)] {
            if sequence == 2 {
                record.phase = hebbian_contracts::CommandPhase::Finished {
                    completion: hebbian_contracts::CommandCompletion {
                        finished_unix_nano: hebbian_contracts::UnixNanos(2_500_000_000),
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

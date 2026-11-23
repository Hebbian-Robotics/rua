//! A model loop's live telemetry: unix timestamps, trace
//! spans, plan, notes, usage and the typed run events.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Maximum number of run events and spans the station can replay per connection.
pub const MAX_RUN_REPLAY_RECORDS: usize = 4096;

/// Unix nanoseconds, written as a decimal string because they outgrow
/// JavaScript's exact integers (as OTLP/JSON writes 64-bit integers).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct UnixNanos(pub u64);

impl Serialize for UnixNanos {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0.to_string())
    }
}

impl<'de> Deserialize<'de> for UnixNanos {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        if !text.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(serde::de::Error::custom(
                "expected decimal Unix nanoseconds",
            ));
        }
        text.parse().map(Self).map_err(serde::de::Error::custom)
    }
}

impl JsonSchema for UnixNanos {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "UnixNanos".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({"type": "string", "pattern": "^[0-9]+$"})
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
pub enum SpanStatusCode {
    #[default]
    #[serde(rename = "STATUS_CODE_UNSET")]
    Unset,
    #[serde(rename = "STATUS_CODE_OK")]
    Ok,
    #[serde(rename = "STATUS_CODE_ERROR")]
    Error,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
pub struct SpanStatus {
    #[serde(default)]
    pub code: SpanStatusCode,
    #[serde(default)]
    pub message: Option<String>,
}

/// One span of a model loop's trace, in OpenTelemetry's terms.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TraceSpan {
    pub trace_id: String,
    pub span_id: String,
    #[serde(default)]
    pub parent_span_id: Option<String>,
    pub name: String,
    pub start_time_unix_nano: UnixNanos,
    pub end_time_unix_nano: UnixNanos,
    #[serde(default)]
    pub attributes: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    pub status: SpanStatus,
}

impl TraceSpan {
    /// A span still running, which the viewer draws up to now.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.attributes.get("hebbian.active") == Some(&serde_json::Value::Bool(true))
    }

    #[must_use]
    pub fn is_run(&self) -> bool {
        self.attributes.get("hebbian.kind") == Some(&serde_json::Value::from("run"))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PlanStepStatus {
    Pending,
    Active,
    Done,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PlanStep {
    pub text: String,
    pub status: PlanStepStatus,
}

/// A model run's token use so far.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RunUsage {
    pub run_id: String,
    pub turns: u64,
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_tokens: u64,
}

/// What the model keeps in mind: its task, its plan and its notes.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
pub struct RunMemory {
    pub task: Option<String>,
    pub plan: Option<Vec<PlanStep>>,
    pub notes: Vec<String>,
}

/// A run event the server acts on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum KnownRunEvent {
    #[schemars(title = "RunStartedEvent")]
    RunStarted {
        #[serde(default)]
        task: Option<String>,
    },
    #[schemars(title = "PlanUpdateEvent")]
    PlanUpdate { steps: Vec<PlanStep> },
    #[schemars(title = "NotesUpdateEvent")]
    NotesUpdate { notes: Vec<String> },
    #[schemars(title = "UsageUpdateEvent")]
    UsageUpdate { usage: RunUsage },
}

/// An event a model loop reports: one the server acts on, or any other kept
/// as it came.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum RunEvent {
    Known(KnownRunEvent),
    Other(serde_json::Value),
}

/// A set of model-loop trace span updates.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RunSpansRequest {
    pub spans: Vec<TraceSpan>,
}

/// The model loop's live telemetry, as the viewer draws it.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
pub struct LiveRun {
    pub revision: u64,
    pub spans: Vec<TraceSpan>,
    pub usage: Option<RunUsage>,
    pub memory: RunMemory,
}

impl UnixNanos {
    #[must_use]
    pub fn as_unix_seconds(self) -> f64 {
        std::time::Duration::from_nanos(self.0).as_secs_f64()
    }

    /// The current time.
    #[must_use]
    pub fn now() -> Self {
        Self(crate::unix_nanoseconds())
    }
}

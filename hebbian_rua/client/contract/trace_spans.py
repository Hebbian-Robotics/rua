"""Trace spans: OpenTelemetry's span model, recorded on /trace/spans in recording MCAPs.

Each span is one step of a run with a start and an end: the run itself, an
observation, a model call, a tool call, a station command. They nest through
parent ids. Fields follow OTLP/JSON (hex ids, Unix nanoseconds) and attribute
names follow OpenTelemetry's semantic conventions where one exists (gen_ai.*
for model and tool calls); the rest are hebbian.*. So a run's spans can be
handed to any OpenTelemetry backend as they are.

The model loop (examples/typescript/agent) traces with the OpenTelemetry SDK and
posts finished spans to the station; it sends a W3C `traceparent` with
each command, so the recorder's command spans are children of the tool call
that sent them.
"""

from enum import StrEnum
from typing import Annotated

from pydantic import Field, JsonValue

from hebbian_rua.client.contract.wire_types import CamelCaseModel

TRACE_SPANS_TOPIC = "/trace/spans"
# What a span is, for the viewer's lanes: run, observe, model, tool, command.
KIND_ATTRIBUTE = "hebbian.kind"

# Unix nanoseconds are decimal strings on the wire, as in OTLP/JSON.
UnixNanoseconds = Annotated[str, Field(pattern=r"^[0-9]+$")]
HexTraceId = Annotated[str, Field(pattern=r"^[0-9a-f]{32}$")]
HexSpanId = Annotated[str, Field(pattern=r"^[0-9a-f]{16}$")]


class SpanStatusCode(StrEnum):
    UNSET = "STATUS_CODE_UNSET"
    OK = "STATUS_CODE_OK"
    ERROR = "STATUS_CODE_ERROR"


class SpanStatus(CamelCaseModel):
    code: SpanStatusCode = SpanStatusCode.UNSET
    message: str | None = None


class TraceSpan(CamelCaseModel):
    trace_id: HexTraceId
    span_id: HexSpanId
    parent_span_id: HexSpanId | None = None
    name: str
    start_time_unix_nano: UnixNanoseconds
    end_time_unix_nano: UnixNanoseconds
    attributes: dict[str, JsonValue] = Field(default_factory=dict)
    status: SpanStatus = Field(default_factory=SpanStatus)

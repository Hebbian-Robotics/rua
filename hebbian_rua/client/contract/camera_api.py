"""Camera headers and the model loop's run memory and usage payloads."""

from typing import Literal

from pydantic import Field

from hebbian_rua.client.contract.trace_spans import TraceSpan
from hebbian_rua.client.contract.wire_types import CamelCaseModel

# Response header on a camera frame: when it was captured, in Unix nanoseconds.
CAPTURED_AT_HEADER = "X-Captured-At-Unix-Ns"


class RunUsage(CamelCaseModel):
    run_id: str
    turns: int
    input_tokens: int
    cached_input_tokens: int
    output_tokens: int
    reasoning_tokens: int


class PlanStep(CamelCaseModel):
    text: str
    status: Literal["pending", "active", "done", "failed"]


class RunMemory(CamelCaseModel):
    task: str | None = None
    plan: list[PlanStep] | None = None
    notes: list[str] = Field(default_factory=list)


class LiveRun(CamelCaseModel):
    revision: int
    spans: list[TraceSpan]
    usage: RunUsage | None
    memory: RunMemory = Field(default_factory=RunMemory)

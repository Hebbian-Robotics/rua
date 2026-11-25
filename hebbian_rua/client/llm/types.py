"""The four things that pass between a model and the robot, free of any LLM SDK."""

import json
from collections.abc import Callable
from dataclasses import dataclass, field
from typing import Literal

from pydantic import JsonValue


@dataclass(frozen=True)
class SequenceImage:
    """A JPEG with client retrieval timing, rather than synchronized robot state."""

    camera: str
    jpeg: bytes
    retrieved_between_unix_seconds: tuple[float, float]
    elapsed_seconds: float
    boundary: Literal["start", "motion", "step", "end"]


@dataclass
class SequenceImageHistory:
    """Visual evidence for one model call, including gaps and retention loss."""

    call_id: str
    images: list[SequenceImage] = field(default_factory=list)
    missed_samples: int = 0
    truncated: bool = False


@dataclass(frozen=True)
class RobotToolSpec:
    """A tool as every function-calling API describes one: a name, a description
    written for the model, and a JSON schema for its arguments."""

    name: str
    description: str
    parameters_schema: dict[str, JsonValue]


@dataclass(frozen=True)
class RobotToolCall:
    """A tool call as the model made it. `arguments` is the parsed JSON object,
    unvalidated: RobotTools.execute validates it."""

    call_id: str
    name: str
    arguments: object


@dataclass(frozen=True)
class ModelContent:
    """Provider-neutral content selected for the model."""

    text: str
    cameras: dict[str, bytes] = field(default_factory=dict)


@dataclass(frozen=True)
class RobotOutput:
    """Station data and context passed through transforms before rendering.

    Payloads stay in original SI units until the final renderer.

    Tool results carry their call and requested images or sequence history;
    observations carry the selected arm, notes, camera JPEGs and retrieval
    intervals. Formatters must not mutate this data.
    """

    kind: Literal["result", "observation"]
    data: object
    call: RobotToolCall | None = None
    cameras: dict[str, bytes] = field(default_factory=dict)
    selected_arm: str | None = None
    notes: tuple[str, ...] = ()
    missing_optional_cameras: tuple[str, ...] = ()
    camera_intervals: dict[str, tuple[float, float]] = field(default_factory=dict)
    sequence_images: SequenceImageHistory | None = None


type OutputFormatter = Callable[[RobotOutput], ModelContent]


@dataclass(frozen=True)
class RobotToolResult:
    """What to send back to the model for one call. `is_error` marks an outcome
    the model should inspect, including a limited move or a trip;
    `halt` marks a hard fault that powered off the motors, after which a loop
    should stop rather than ask the model again."""

    call: RobotToolCall
    text: str
    is_error: bool
    halt: bool = False
    interrupted: bool = False

    # INVARIANT: Full station payload in original units, or the original error text.
    data: object = field(kw_only=True)
    cameras: dict[str, bytes] = field(default_factory=dict, kw_only=True)
    sequence_images: SequenceImageHistory | None = field(default=None, kw_only=True)
    observed_revisions: dict[str, int] | None = field(default=None, kw_only=True)

    @property
    def model_data(self) -> object:
        """Parse the formatted model text, independently of the full payload."""
        try:
            return json.loads(self.text)
        except (ValueError, TypeError):
            return self.text


@dataclass(frozen=True)
class RobotObservation:
    """What the model sees: the arms' state as text and a JPEG per camera."""

    text: str
    cameras: dict[str, bytes] = field(default_factory=dict)
    data: object = field(kw_only=True)
    revisions: dict[str, int] = field(default_factory=dict)

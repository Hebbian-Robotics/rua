"""The four things that pass between a model and the robot, free of any LLM SDK."""

import json
from collections.abc import Callable
from dataclasses import dataclass, field
from typing import Any, Literal


@dataclass(frozen=True)
class RobotToolSpec:
    """A tool as every function-calling API describes one: a name, a description
    written for the model, and a JSON schema for its arguments."""

    name: str
    description: str
    parameters_schema: dict[str, Any]


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
    """Full, unrounded station data supplied to an output formatter.

    Tool results carry their call; observations carry the selected arm, notes,
    camera JPEGs and retrieval intervals. Formatters must not mutate this data.
    """

    kind: Literal["result", "observation"]
    data: object
    call: RobotToolCall | None = None
    cameras: dict[str, bytes] = field(default_factory=dict)
    selected_arm: str | None = None
    notes: tuple[str, ...] = ()
    missing_optional_cameras: tuple[str, ...] = ()
    camera_intervals: dict[str, tuple[float, float]] = field(default_factory=dict)


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

    raw_data: object | None = None

    @property
    def data(self) -> object:
        """Full station payload in original units, or the original error text."""
        return self.raw_data if self.raw_data is not None else self.model_data

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
    data: object | None = None

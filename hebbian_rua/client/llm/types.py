"""The four things that pass between a model and the robot, free of any LLM SDK."""

import json
from dataclasses import dataclass, field
from typing import Any


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

    @property
    def data(self) -> object:
        """Structured JSON payload when available, otherwise the message text."""
        try:
            return json.loads(self.text)
        except (ValueError, TypeError):
            return self.text


@dataclass(frozen=True)
class RobotObservation:
    """What the model sees: the arms' state as text and a JPEG per camera."""

    text: str
    cameras: dict[str, bytes] = field(default_factory=dict)

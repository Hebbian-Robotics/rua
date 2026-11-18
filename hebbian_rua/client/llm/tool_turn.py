"""Shared execution outcomes; provider SDKs stay in their own modules."""

import base64
import json
from dataclasses import dataclass

from hebbian_rua.client.llm.types import RobotToolResult


@dataclass(frozen=True)
class RobotToolTurn[Message]:
    """Append messages to the conversation, then check results before continuing.

    Messages include the original model response and tool results, preserving
    reasoning and signatures. No model requests are made by an adapter.
    """

    messages: list[Message]
    results: list[RobotToolResult]

    @property
    def should_stop(self) -> bool:
        """Stop on a hard fault or an execution with an uncertain outcome."""
        return any(result.halt or result.interrupted for result in self.results)


def decode_arguments(arguments: str) -> object:
    """Let shared robot validation report malformed JSON as a tool error."""
    try:
        return json.loads(arguments)
    except json.JSONDecodeError:
        return arguments


def encode_camera(jpeg: bytes) -> str:
    return base64.standard_b64encode(jpeg).decode("ascii")

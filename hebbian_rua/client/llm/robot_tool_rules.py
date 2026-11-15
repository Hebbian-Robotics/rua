"""Shared tool parsing, resource rules, and result formatting."""

from collections.abc import Callable
from dataclasses import dataclass, field
from typing import assert_never

from pydantic import ValidationError

from hebbian_rua.client.contract.station_api import (
    CommandOutcome,
    CommandRecord,
    CommandRefused,
)
from hebbian_rua.client.llm.response_format import result_from_data
from hebbian_rua.client.llm.tools import (
    ROBOT_TOOL_INPUTS,
    RobotToolInput,
)
from hebbian_rua.client.llm.types import (
    RobotObservation,
    RobotToolCall,
    RobotToolResult,
)
from hebbian_rua.client.station_client import (
    CameraJpeg,
    CommandResult,
    command_record_was_limited,
)


def outcome_is_error(outcome: CommandOutcome | None) -> bool:
    """Whether the model is told the command did not do what it asked; a command
    with no outcome yet did not finish."""
    match outcome:
        case CommandOutcome.completed:
            return False
        case (
            None
            | CommandOutcome.stopped
            | CommandOutcome.tripped
            | CommandOutcome.timed_out
            | CommandOutcome.stopped_short
            | CommandOutcome.superseded
            | CommandOutcome.faulted
            | CommandOutcome.interrupted
            | CommandOutcome.failed
        ):
            return True
        case _ as unreachable:
            assert_never(unreachable)


def outcome_halts_the_loop(outcome: CommandOutcome | None) -> bool:
    """A hard fault powers off the motors, so a loop ends rather than ask the model
    again. A trip does not: the arm holds, and the model sees the result and the
    frames."""
    match outcome:
        case CommandOutcome.faulted:
            return True
        case (
            None
            | CommandOutcome.completed
            | CommandOutcome.stopped
            | CommandOutcome.tripped
            | CommandOutcome.timed_out
            | CommandOutcome.stopped_short
            | CommandOutcome.superseded
            | CommandOutcome.interrupted
            | CommandOutcome.failed
        ):
            return False
        case _ as unreachable:
            assert_never(unreachable)


# Runs one call: a loop can wrap it to trace or log each call.
type CallWrapper = Callable[
    [RobotToolCall, Callable[[], RobotToolResult]], RobotToolResult
]


@dataclass(frozen=True)
class CapturedObservation:
    """An observation with the capture details a recorder may want to keep."""

    observation: RobotObservation
    captures: dict[str, CameraJpeg] = field(default_factory=dict)


def records_result(
    call: RobotToolCall, records: list[CommandRecord], *, grouped: bool = False
) -> RobotToolResult:
    """Report incomplete or limited commands as errors; group moves by status."""
    limited = any(command_record_was_limited(record) for record in records)
    payloads = [record.model_dump(mode="json") for record in records]
    if grouped:
        outcomes = {
            record.completion.outcome if record.completion else None
            for record in records
        }
        if outcomes == {CommandOutcome.completed} and limited:
            status = "limited"
        elif outcomes == {CommandOutcome.completed}:
            status = "reached"
        elif CommandOutcome.stopped_short in outcomes:
            status = "stopped_short"
        else:
            status = "partial"
        payload = {
            "group_id": records[0].group_id,
            "status": status,
            "commands": payloads,
        }
    elif len(records) == 1:
        payload = payloads[0]
    else:
        payload = payloads
    return result_from_data(
        call,
        payload,
        is_error=limited
        or any(
            outcome_is_error(record.completion.outcome if record.completion else None)
            for record in records
        ),
        halt=any(
            outcome_halts_the_loop(
                record.completion.outcome if record.completion else None
            )
            for record in records
        ),
        interrupted=any(
            record.completion is not None
            and record.completion.outcome is CommandOutcome.interrupted
            for record in records
        ),
    )


def command_results(
    call: RobotToolCall, results: list[CommandResult], *, grouped: bool
) -> RobotToolResult:
    """The station's answers as a tool result; a refusal is an error the model
    sees, marked in the text too since not every provider has an error flag."""
    refusal = results[0].record
    if isinstance(refusal, CommandRefused):
        return result_from_data(
            call,
            refusal.model_dump(mode="json"),
            is_error=True,
            halt=refusal.reason.value == "faulted",
            interrupted=refusal.reason.value
            in {"stale_revision", "interlocked", "lease_held_elsewhere"},
        )
    records = [
        result.record
        for result in results
        if not isinstance(result.record, CommandRefused)
    ]
    return records_result(call, records, grouped=grouped)


class RobotToolRules:
    """Parsing and resource rules shared by station adapters."""

    arm: str

    def parse(self, call: RobotToolCall) -> RobotToolInput | RobotToolResult:
        """The call's validated input, or the error result to send back instead."""
        model = ROBOT_TOOL_INPUTS.get(call.name)
        if model is None:
            return RobotToolResult(call, f"Unknown tool {call.name}", is_error=True)
        if (
            call.name in {"set_arm_gripper", "stop"}
            and isinstance(call.arguments, dict)
            and "kind" in call.arguments
        ):
            return RobotToolResult(
                call, "Invalid input: kind is supplied by the tool name", is_error=True
            )
        try:
            return model.model_validate(call.arguments)
        except ValidationError as error:
            return RobotToolResult(call, f"Invalid input: {error}", is_error=True)

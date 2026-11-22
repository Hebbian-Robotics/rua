"""Shared tool parsing, resource rules, and result formatting."""

from collections.abc import Callable
from dataclasses import dataclass, field
from typing import assert_never

from hebbian_rua.client.contract.station_api import (
    CommandOutcome,
    CommandRecord,
    CommandRefused,
)
from hebbian_rua.client.llm.generated_robot_tools import ROBOT_TOOL_INPUT_UNITS
from hebbian_rua.client.llm.response_format import result_from_data
from hebbian_rua.client.llm.tool_catalog import (
    RobotToolConfiguration,
    parse_sequence_arguments,
)
from hebbian_rua.client.llm.tools import (
    ROBOT_TOOL_INPUTS,
    RobotToolInput,
)
from hebbian_rua.client.llm.types import (
    RobotObservation,
    RobotToolCall,
    RobotToolResult,
)
from hebbian_rua.client.station_types import (
    CameraJpeg,
    CommandResult,
    command_record_outcome,
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
    outcomes = {command_record_outcome(record) for record in records}
    if grouped:
        if outcomes == {CommandOutcome.completed} and limited:
            status = "limited"
        elif outcomes == {CommandOutcome.completed}:
            status = "reached"
        elif CommandOutcome.stopped_short in outcomes:
            status = "stopped_short"
        else:
            status = "partial"
        first_command = records[0].command
        payload = {
            "group_id": first_command.group_id
            if first_command.kind == "move"
            else None,
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
        is_error=limited or any(outcome_is_error(outcome) for outcome in outcomes),
        halt=any(outcome_halts_the_loop(outcome) for outcome in outcomes),
        interrupted=CommandOutcome.interrupted in outcomes,
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


_MODEL_INPUT_CONVERSIONS = {
    unit["model_field"]: (unit["station_field"], unit["scale_to_station"])
    for unit in ROBOT_TOOL_INPUT_UNITS
}
_STATION_UNIT_FIELDS = {
    station_field for station_field, _ in _MODEL_INPUT_CONVERSIONS.values()
}


def convert_model_arguments_to_station_units(value: object) -> object:
    """Convert model millimeters/degrees to station meters/radians."""
    if isinstance(value, list):
        return [convert_model_arguments_to_station_units(item) for item in value]
    if not isinstance(value, dict):
        return value

    def scale_model_input_numbers(item: object, factor: float) -> object:
        if isinstance(item, list):
            return [scale_model_input_numbers(element, factor) for element in item]
        return (
            item * factor
            if isinstance(item, (int, float)) and not isinstance(item, bool)
            else item
        )

    converted = {}
    for key, item in value.items():
        if key in _STATION_UNIT_FIELDS:
            raise ValueError(f"Use explicit model units for {key}")
        if key in _MODEL_INPUT_CONVERSIONS:
            station_field, factor = _MODEL_INPUT_CONVERSIONS[key]
            converted[station_field] = scale_model_input_numbers(item, factor)
        elif key in {"arms", "targets"} and isinstance(item, dict):
            converted[key] = {
                arm: convert_model_arguments_to_station_units(target)
                for arm, target in item.items()
            }
        else:
            converted[key] = convert_model_arguments_to_station_units(item)
    return converted


class RobotToolRules:
    """Parsing and resource rules shared by station adapters."""

    arm: str
    configuration: RobotToolConfiguration

    def parse(self, call: RobotToolCall) -> RobotToolInput | RobotToolResult:
        """The call's validated input, or the error result to send back instead."""
        model = ROBOT_TOOL_INPUTS.get(call.name)
        if model is None:
            return result_from_data(call, f"Unknown tool {call.name}", is_error=True)
        if call.name not in self.configuration.tools:
            return result_from_data(
                call, f"Tool {call.name} is not available", is_error=True
            )
        if (
            call.name in {"set_arm_gripper", "stop"}
            and isinstance(call.arguments, dict)
            and "kind" in call.arguments
        ):
            return result_from_data(
                call, "Invalid input: kind is supplied by the tool name", is_error=True
            )
        try:
            if call.name == "action_sequence":
                return parse_sequence_arguments(
                    convert_model_arguments_to_station_units(call.arguments),
                    self.configuration,
                    self.arm,
                )
            return model.model_validate(
                convert_model_arguments_to_station_units(call.arguments)
            )
        except ValueError as error:
            return result_from_data(call, f"Invalid input: {error}", is_error=True)

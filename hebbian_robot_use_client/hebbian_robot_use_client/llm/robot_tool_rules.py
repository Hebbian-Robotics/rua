"""Shared tool parsing, resource rules, and result formatting."""

import json
from collections.abc import Callable
from dataclasses import dataclass, field
from typing import assert_never

from pydantic import BaseModel, ConfigDict, TypeAdapter, ValidationError

from hebbian_robot_use_client.contract.station_api import (
    ArmMode,
    ArmState,
    CommandOutcome,
    CommandRecord,
    CommandRefused,
    PoseDescription,
    StationState,
)
from hebbian_robot_use_client.llm.response_format import model_response
from hebbian_robot_use_client.llm.tools import (
    ROBOT_TOOL_INPUTS,
    RobotToolInput,
)
from hebbian_robot_use_client.llm.types import (
    RobotObservation,
    RobotToolCall,
    RobotToolResult,
)
from hebbian_robot_use_client.station_client import (
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


class ArmSummary(BaseModel):
    """One arm as the model sees it: the state without diagnostics."""

    model_config = ConfigDict(extra="forbid", frozen=True)

    mode: ArmMode
    sampled_at_unix_seconds: float | None
    fault: str | None
    joint_names: list[str]
    measured_joints: list[float]
    # The tool's position, quaternion, grasp-terms orientation and model points.
    tool: PoseDescription | None
    max_joint_speed: float
    # 0 closed to 1 open; none without a gripper.
    gripper_opening: float | None


ARM_SUMMARIES_ADAPTER: TypeAdapter[dict[str, ArmSummary]] = TypeAdapter(
    dict[str, ArmSummary]
)


def arm_summary(state: StationState, arm_state: ArmState) -> ArmSummary:
    opening = arm_state.gripper_opening
    return ArmSummary(
        mode=arm_state.mode,
        sampled_at_unix_seconds=arm_state.sampled_at_unix_seconds,
        fault=arm_state.fault,
        joint_names=state.joint_names,
        measured_joints=[round(value, 4) for value in arm_state.measured_joints],
        tool=arm_state.tool,
        max_joint_speed=state.max_joint_speed,
        gripper_opening=None if opening is None else round(opening, 3),
    )


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
    payloads = [
        model_response(record.model_dump(mode="json", exclude={"checkpoints"}))
        for record in records
    ]
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
        text = json.dumps(
            {
                "group_id": records[0].group_id,
                "status": status,
                "commands": payloads,
            }
        )
    elif len(records) == 1:
        text = json.dumps(payloads[0], separators=(",", ":"))
    else:
        text = json.dumps(payloads)
    return RobotToolResult(
        call,
        text,
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
        return RobotToolResult(
            call,
            json.dumps(model_response(refusal.model_dump(mode="json"))),
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

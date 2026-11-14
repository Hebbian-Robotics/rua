"""Types shared by station clients and their callers."""

from collections.abc import Generator
from contextlib import contextmanager
from contextvars import ContextVar
from dataclasses import dataclass
from enum import StrEnum
from typing import NewType, Self

from hebbian_robot_use_client.contract.station_api import (
    ApproveRecoveryCommand,
    ClearFaultCommand,
    CommandOutcome,
    CommandRecord,
    CommandRefused,
    DisableCommand,
    EnableCommand,
    FollowPathCommand,
    LeaseStatus,
    MoveToJointsCommand,
    MoveToPoseCommand,
    StationSetGripperCommand,
    StationStreamJointsCommand,
    StopCommand,
    TraceContext,
)

# The station's HTTP API, event stream and traces.
DEFAULT_SERVER = "http://127.0.0.1:8792"
# The media service (`hebbian-robot-use-server serve --media-address`): camera snapshots.
DEFAULT_MEDIA_SERVER = "http://127.0.0.1:8781"
CAMERA_TIMEOUT_SECONDS = 30
RUNS_TIMEOUT_SECONDS = 30

WorldPositionMeters = NewType("WorldPositionMeters", list[float])
JointSpeedRadiansPerSecond = NewType("JointSpeedRadiansPerSecond", float)


type StationCommand = (
    EnableCommand
    | ApproveRecoveryCommand
    | DisableCommand
    | StopCommand
    | ClearFaultCommand
    | MoveToJointsCommand
    | MoveToPoseCommand
    | FollowPathCommand
    | StationStreamJointsCommand
    | StationSetGripperCommand
)


# The W3C trace context commands sent from this context carry, so each command's
# record names the tool call that sent it. The client does not depend on a
# tracing library: whoever opens the span (a tool call) sets this for its block.
trace_context_for_commands: ContextVar[TraceContext | None] = ContextVar(
    "trace_context_for_commands", default=None
)


@contextmanager
def commands_parented_to(
    traceparent: str | None, tracestate: str | None = None
) -> Generator[None]:
    """Commands sent inside this block carry the W3C trace context of the span
    that wraps it. No traceparent (an unsampled or absent span) means none."""
    token = trace_context_for_commands.set(
        None
        if traceparent is None
        else TraceContext(traceparent=traceparent, tracestate=tracestate)
    )
    try:
        yield
    finally:
        trace_context_for_commands.reset(token)


class ResultOutcome(StrEnum):
    """What became of a command. `COMPLETED` is the only success."""

    COMPLETED = CommandOutcome.completed.value
    STOPPED = CommandOutcome.stopped.value
    # A safety limit stopped the arm.
    TRIPPED = CommandOutcome.tripped.value
    TIMED_OUT = CommandOutcome.timed_out.value
    STOPPED_SHORT = CommandOutcome.stopped_short.value
    SUPERSEDED = CommandOutcome.superseded.value
    FAULTED = CommandOutcome.faulted.value
    FAILED = CommandOutcome.failed.value
    # The station refused the command: the arm never moved.
    REJECTED = "rejected"
    # The station has not reported an outcome.
    UNFINISHED = "unfinished"


def command_record_was_limited(record: CommandRecord) -> bool:
    return record.pose_limited is not None or record.path_limited is not None


@dataclass(frozen=True)
class CommandResult:
    """A command outcome, its detail, and the station's full typed answer.

    A completed motion finished its plan, which may have been limited before the
    requested target. Inspect pose_limited and path_limited on
    an accepted CommandRecord when reaching the requested target matters.
    A refused command has a CommandRefused record and did not move the arm.
    """

    outcome: ResultOutcome
    detail: str | None
    record: CommandRecord | CommandRefused

    @classmethod
    def of(cls, answer: CommandRecord | CommandRefused) -> Self:
        if isinstance(answer, CommandRefused):
            return cls(
                ResultOutcome.REJECTED,
                f"{answer.reason.value}: {answer.detail}",
                answer,
            )
        outcome = (
            ResultOutcome.UNFINISHED
            if answer.completion is None
            else ResultOutcome(answer.completion.outcome.value)
        )
        return cls(
            outcome, answer.completion.detail if answer.completion else None, answer
        )

    @property
    def completed(self) -> bool:
        return self.outcome is ResultOutcome.COMPLETED

    @property
    def was_limited(self) -> bool:
        """Whether the accepted plan was limited; a refused command has no plan."""
        return isinstance(self.record, CommandRecord) and command_record_was_limited(
            self.record
        )


@dataclass(frozen=True)
class CameraJpeg:
    """A camera's frame, with when it was fetched and when the camera took it."""

    jpeg: bytes
    retrieved_between_unix_seconds: tuple[float, float]
    # None when the station did not say; frames from LiveKit cameras carry it.
    captured_at_unix_ns: int | None


class LeaseLost(Exception):
    """The client no longer holds the lease."""


class LeaseHeldElsewhere(Exception):
    """Someone else holds the lease on the arms, and this client did not take it."""

    def __init__(self, held_by: LeaseStatus | None) -> None:
        super().__init__(
            "The lease is held by someone else"
            if held_by is None
            else f"The lease is held by {held_by.holder.value} '{held_by.label}'"
        )
        self.held_by = held_by

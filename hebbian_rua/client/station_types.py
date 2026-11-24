"""Types shared by station clients and their callers."""

from collections.abc import Generator
from contextlib import contextmanager
from contextvars import ContextVar
from dataclasses import dataclass
from enum import StrEnum
from typing import NewType, assert_never

from hebbian_rua.client.contract.arm_commands import StationCommand as StationCommand
from hebbian_rua.client.contract.station_api import (
    CameraListing,
    CommandOutcome,
    CommandRecord,
    CommandRefused,
    LeaseStatus,
    TraceContext,
)

# The station's HTTP API, event stream and traces.
DEFAULT_SERVER = "http://127.0.0.1:8792"
# The media service (`hebbian-rua-server serve --media-address`): camera snapshots.
DEFAULT_MEDIA_SERVER = "http://127.0.0.1:8781"
CAMERA_TIMEOUT_SECONDS = 30
RUNS_TIMEOUT_SECONDS = 30

WorldPositionMeters = NewType("WorldPositionMeters", list[float])
JointSpeedRadiansPerSecond = NewType("JointSpeedRadiansPerSecond", float)


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
    INTERRUPTED = CommandOutcome.interrupted.value
    # The station refused the command: the arm never moved.
    REJECTED = "rejected"
    # The station has not reported an outcome.
    UNFINISHED = "unfinished"

    @classmethod
    def from_command_outcome(cls, outcome: CommandOutcome) -> "ResultOutcome":
        match outcome:
            case CommandOutcome.completed:
                return cls.COMPLETED
            case CommandOutcome.stopped:
                return cls.STOPPED
            case CommandOutcome.tripped:
                return cls.TRIPPED
            case CommandOutcome.timed_out:
                return cls.TIMED_OUT
            case CommandOutcome.stopped_short:
                return cls.STOPPED_SHORT
            case CommandOutcome.superseded:
                return cls.SUPERSEDED
            case CommandOutcome.faulted:
                return cls.FAULTED
            case CommandOutcome.failed:
                return cls.FAILED
            case CommandOutcome.interrupted:
                return cls.INTERRUPTED
            case _ as unreachable:
                assert_never(unreachable)


def command_record_outcome(record: CommandRecord) -> CommandOutcome | None:
    phase = record.phase
    match phase.kind:
        case "pending":
            return None
        case "finished":
            return CommandOutcome(phase.completion.outcome.value)
        case "not_started":
            return CommandOutcome(phase.outcome.value)
        case "outcome_unknown":
            return CommandOutcome.interrupted
        case _ as unreachable:
            assert_never(unreachable)


def command_record_finished_unix_nano(record: CommandRecord) -> int | None:
    phase = record.phase
    match phase.kind:
        case "pending":
            return None
        case "finished":
            return int(phase.completion.finished_unix_nano)
        case "not_started" | "outcome_unknown":
            return int(phase.finished_unix_nano)
        case _ as unreachable:
            assert_never(unreachable)


def command_record_detail(record: CommandRecord) -> str | None:
    phase = record.phase
    match phase.kind:
        case "pending":
            return None
        case "finished":
            return phase.completion.detail
        case "not_started" | "outcome_unknown":
            return phase.detail
        case _ as unreachable:
            assert_never(unreachable)


def command_record_was_limited(record: CommandRecord) -> bool:
    command = record.command
    return command.kind == "move" and (
        command.pose_limited is not None or command.path_limited is not None
    )


@dataclass(frozen=True)
class CommandResult:
    """A command outcome, its detail, and the station's full typed answer.

    A completed motion finished its plan, which may have been limited before the
    requested target. Inspect pose_limited and path_limited on
    an accepted CommandRecord when reaching the requested target matters.
    A refused command has a CommandRefused record and did not move the arm.
    """

    record: CommandRecord | CommandRefused

    @property
    def outcome(self) -> ResultOutcome:
        if isinstance(self.record, CommandRefused):
            return ResultOutcome.REJECTED
        outcome = command_record_outcome(self.record)
        if outcome is None:
            return ResultOutcome.UNFINISHED
        return ResultOutcome.from_command_outcome(outcome)

    @property
    def detail(self) -> str | None:
        if isinstance(self.record, CommandRefused):
            return f"{self.record.reason.value}: {self.record.detail}"
        return command_record_detail(self.record)

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


class CameraUnavailable(Exception):
    """The media service has no fresh frame for this camera.

    `listed` is what the media service reports. A camera missing from it was never
    published or the name is wrong; one listed as unavailable stopped streaming.
    `last_error` is the latest failure the station saw in the media service's output for
    it, when the station could be asked.
    """

    def __init__(
        self,
        name: str,
        listed: list[CameraListing],
        last_error: str | None = None,
    ) -> None:
        known = (
            ", ".join(
                f"{camera.name} ({'streaming' if camera.available else 'unavailable'})"
                for camera in listed
            )
            if listed
            else "no cameras; check that the media service is running and the "
            "station's --camera device ids are current"
        )
        super().__init__(
            f"camera '{name}' is not streaming (the media service lists {known})"
            + ("" if last_error is None else f"; last error: {last_error}")
        )
        self.name = name
        self.listed = listed
        self.last_error = last_error


class ActionSequenceUnavailable(LookupError):
    """The sequence identifier is unknown or its outcome has expired."""

    def __init__(self, action_sequence_id: str) -> None:
        self.action_sequence_id = action_sequence_id
        super().__init__(
            f"Action sequence {action_sequence_id} is unknown or its result expired"
        )


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

"""Reconnect and retain station observations from its HTTP event stream."""

import asyncio
import logging
from collections import OrderedDict
from collections.abc import AsyncIterator, Callable
from typing import Annotated, assert_never

import httpx2
from pydantic import Field, TypeAdapter, ValidationError

from hebbian_rua.client.contract.station_api import (
    ActionSequenceRecord,
    ActionSequenceState,
    ArmFrame,
    ArmRecord,
    CommandRecord,
    Event1,
    Event2,
    Frame,
    Mode,
    Record,
    Rejected,
    Reply,
    Resync,
    RunEventRecord,
    RunMessage,
    RunResync,
    RunSpanRecord,
    SnapshotTaken,
    State,
    StateMessage,
    StationState,
    Tripped,
)
from hebbian_rua.client.station_events import (
    StationStreamEvent,
    stream_station_events,
)
from hebbian_rua.client.station_types import command_record_finished_at

log = logging.getLogger(__name__)
# How long to wait for a connection, or for the first state.
CONNECT_TIMEOUT_SECONDS = 10.0
# How long a request, or an awaited state, waits before it is given up on.
REPLY_TIMEOUT_SECONDS = 30.0
# How long a command's outcome is awaited across a lost connection.
DISCONNECT_PATIENCE_SECONDS = 30.0
# How long the event stream may stay silent before it is reconnected. The
# station sends a keepalive comment at least every 15 seconds while idle.
EVENT_STREAM_READ_TIMEOUT_SECONDS = 30.0
RETAINED_RECORDS = 10_000


# What a model loop's run telemetry reports: events and spans in order, a
# resync when the cursor fell out of the retained records, and the live snapshot.
type RunUpdate = RunEventRecord | RunSpanRecord | RunResync | RunMessage

_STATION_EVENT_MESSAGE = TypeAdapter(
    Annotated[
        ArmFrame | StateMessage | ActionSequenceState | RunUpdate,
        Field(discriminator="kind"),
    ]
)


class MessageRefused(Exception):
    """The station could not act on a message, such as one naming an unknown arm."""

    def __init__(self, detail: str) -> None:
        super().__init__(detail)
        self.detail = detail


class StationConnection:
    """A reconnecting station connection owned by one asyncio event loop."""

    def __init__(self, server: str, *, run_after: int | None = None) -> None:
        """`run_after` resumes the run telemetry after that sequence number;
        0 replays everything retained, and None starts at the newest."""
        self._url = server.rstrip("/")
        self._event_client = httpx2.AsyncClient(base_url=server, timeout=None)
        self._event_reader: asyncio.Task[None] | None = None
        self._run_after = run_after
        self._run_subscribers: set[asyncio.Queue[RunUpdate | None]] = set()
        self._condition = asyncio.Condition()
        self._connected = False
        self._closed = False
        self._reader_error: Exception | None = None
        self._disconnected_at: float | None = None
        self._state: StationState | None = None
        self._state_count = 0
        self._action_sequence: ActionSequenceRecord | None = None
        self._action_sequence_revision = 0
        self._records: OrderedDict[str, ArmRecord] = OrderedDict()
        self.lease_lost = asyncio.Event()
        self._last_action_sequence: dict[str, int] = {}

    async def start(self) -> None:
        if self._closed:
            raise ConnectionError("The station connection is closed")
        if self._event_reader is None:
            self._disconnected_at = asyncio.get_running_loop().time()
            self._event_reader = asyncio.create_task(self._read_events())

    async def close(self) -> None:
        self._closed = True
        if self._event_reader is not None:
            self._event_reader.cancel()
            await asyncio.gather(self._event_reader, return_exceptions=True)
        await self._event_client.aclose()
        async with self._condition:
            self._condition.notify_all()
        self._end_run_subscribers()

    def _end_run_subscribers(self) -> None:
        for queue in self._run_subscribers:
            queue.put_nowait(None)

    async def run_telemetry(self) -> AsyncIterator[RunUpdate]:
        """Run events, spans, resyncs and snapshots from the first message on.

        The stream resumes after `run_after`, each record once and in order.
        It ends when the connection is closed.
        """
        queue: asyncio.Queue[RunUpdate | None] = asyncio.Queue()
        self._run_subscribers.add(queue)
        try:
            await self.start()
            while (update := await queue.get()) is not None:
                yield update
        finally:
            self._run_subscribers.discard(queue)

    async def connect(self) -> None:
        """Wait for a usable station state from the event stream."""
        await self.state()

    def change_stamp(self) -> tuple[object, ...]:
        """Identify each arm's mode, revision and interlocks right now."""
        arms = (
            ()
            if self._state is None
            else tuple(
                (
                    arm.arm,
                    arm.mode,
                    arm.revision,
                    tuple(interlock.kind for interlock in arm.interlocks),
                )
                for arm in self._state.arms
            )
        )
        return (arms, self._connected)

    async def wait_for_change(self, observed: tuple[object, ...]) -> None:
        """Wait until an arm's mode, revision or interlocks, or the event stream,
        differs from `observed`.

        Take the stamp before reading the state, so a change in between wakes
        this call.
        """
        async with self._condition:
            await self._condition.wait_for(
                lambda: (
                    self.change_stamp() != observed
                    or self._closed
                    or self._reader_error is not None
                )
            )
            if self._closed or self._reader_error is not None:
                raise ConnectionError(
                    "The station connection is closed"
                ) from self._reader_error

    async def state(self) -> StationState:
        await self.start()
        async with self._condition:
            try:
                async with asyncio.timeout(CONNECT_TIMEOUT_SECONDS):
                    await self._condition.wait_for(
                        lambda: (
                            self._state is not None
                            or self._closed
                            or self._reader_error is not None
                        )
                    )
            except TimeoutError as error:
                raise ConnectionError(f"No station state from {self._url}") from error
            if self._state is None:
                if self._reader_error is not None:
                    raise ConnectionError(
                        f"Connection to {self._url} failed"
                    ) from self._reader_error
                raise ConnectionError("The station connection is closed")
            return self._state

    @property
    def state_count(self) -> int:
        """State frames received so far, across reconnects."""
        return self._state_count

    async def wait_for_state(
        self,
        predicate: Callable[[StationState], bool],
        *,
        after_frame_count: int | None = None,
    ) -> StationState:
        """Wait for a state frame that satisfies `predicate`, for example every arm
        reporting a sample after the station starts, and return it. Checks the
        latest frame first.

        With `after_frame_count` (an earlier `state_count`), only a frame received
        after that count qualifies, so a cached frame that predates an event
        cannot satisfy the wait.

        Raises `TimeoutError` when no frame satisfies it in `REPLY_TIMEOUT_SECONDS`.
        """
        await self.start()

        def satisfied() -> bool:
            return (
                self._state is not None
                and (after_frame_count is None or self._state_count > after_frame_count)
                and predicate(self._state)
            )

        async with self._condition:
            try:
                async with asyncio.timeout(REPLY_TIMEOUT_SECONDS):
                    await self._condition.wait_for(
                        lambda: (
                            satisfied()
                            or self._closed
                            or self._reader_error is not None
                        )
                    )
            except TimeoutError as error:
                raise TimeoutError(
                    "Timed out waiting for the awaited station state"
                ) from error
            if self._state is None or not satisfied():
                raise ConnectionError(
                    "The station connection is closed"
                ) from self._reader_error
            return self._state

    async def action_sequence_updates(
        self,
    ) -> AsyncIterator[ActionSequenceRecord | None]:
        """Yield action_sequence snapshots, including the snapshot after reconnect."""
        await self.start()
        revision = 0
        while True:
            async with self._condition:
                await self._condition.wait_for(
                    lambda revision=revision: (
                        self._action_sequence_revision > revision
                        or self._closed
                        or self._reader_error is not None
                    )
                )
                if self._closed or self._reader_error is not None:
                    raise ConnectionError(
                        "The station connection is closed"
                    ) from self._reader_error
                revision = self._action_sequence_revision
                action_sequence = self._action_sequence
            yield action_sequence

    async def remember(self, arm_record: ArmRecord) -> None:
        async with self._condition:
            self._keep_record(arm_record)
            self._condition.notify_all()

    async def wait_for_outcome(self, command_id: str) -> CommandRecord:
        await self.start()
        async with self._condition:
            while True:
                arm_record = self._records.get(command_id)
                if arm_record is not None and arm_record.record.phase.kind != "pending":
                    await self._condition.wait_for(
                        lambda arm_record=arm_record: (
                            self._state_reflects(arm_record)
                            or self._closed
                            or not self._connected
                        )
                    )
                    return arm_record.record
                if self._closed or self._reader_error is not None:
                    raise ConnectionError(
                        f"The station connection to {self._url} ended"
                    ) from self._reader_error
                if not self._connected:
                    try:
                        disconnected_at = self._disconnected_at
                        deadline = (
                            asyncio.get_running_loop().time()
                            if disconnected_at is None
                            else disconnected_at
                        ) + DISCONNECT_PATIENCE_SECONDS
                        async with asyncio.timeout_at(deadline):
                            await self._condition.wait()
                    except TimeoutError as error:
                        raise ConnectionError(
                            f"{self._url} has been unreachable for too long"
                        ) from error
                else:
                    await self._condition.wait()

    def _state_reflects(self, arm_record: ArmRecord) -> bool:
        """Wait for the command's own arm to report its settled result.

        Commands that finish in one cycle show only the latest on that arm.
        """
        record = arm_record.record
        finished_at = command_record_finished_at(record)
        if self._state is None or finished_at is None:
            return False
        arm = next((arm for arm in self._state.arms if arm.arm == arm_record.arm), None)
        if arm is None or (
            arm.active_command is not None
            and arm.active_command.command_id == record.command_id
        ):
            return False
        if arm.last_command is None:
            return False
        last_finished_at = command_record_finished_at(arm.last_command)
        return last_finished_at is not None and (
            arm.last_command.command_id == record.command_id
            or last_finished_at >= finished_at
        )

    def _keep_record(self, arm_record: ArmRecord) -> None:
        record = arm_record.record
        previous = self._records.get(record.command_id)
        if (
            previous is not None
            and previous.record.phase.kind != "pending"
            and record.phase.kind == "pending"
        ):
            return
        self._records[record.command_id] = arm_record
        self._records.move_to_end(record.command_id)
        while len(self._records) > RETAINED_RECORDS:
            self._records.popitem(last=False)

    async def _read_events(self) -> None:
        try:
            async for event in stream_station_events(
                self._event_client,
                read_timeout_seconds=EVENT_STREAM_READ_TIMEOUT_SECONDS,
            ):
                await self._handle_stream_event(event)
        except Exception as error:
            self._reader_error = error
            log.warning("Station event stream stopped: %s", error)
        finally:
            async with self._condition:
                self._state = None
                self._connected = False
                self.lease_lost.set()
                self._condition.notify_all()
            self._end_run_subscribers()

    async def _handle_stream_event(self, event: StationStreamEvent) -> None:
        """Apply an SSE snapshot or event to the existing station state cache."""
        if event.kind in {"resync_required", "disconnected"} or (
            isinstance(event.data, dict) and event.data.get("kind") == "resync_required"
        ):
            async with self._condition:
                self._state = None
                self._connected = False
                self.lease_lost.set()
                if self._disconnected_at is None:
                    self._disconnected_at = asyncio.get_running_loop().time()
                self._condition.notify_all()
            return
        if event.kind == "snapshot":
            if not isinstance(event.data, dict) or not isinstance(
                event.data.get("items"), list
            ):
                raise ValueError("Invalid station event snapshot")
            async with self._condition:
                self._state = None
                self._records.clear()
                self._last_action_sequence.clear()
                self._condition.notify_all()
            if self._run_after is None:
                run_sequence = event.data.get("run_sequence")
                if not isinstance(run_sequence, int) or run_sequence < 0:
                    raise ValueError("Invalid snapshot run cursor")
                self._run_after = run_sequence
                replay_run_records = False
            else:
                replay_run_records = True
            # Subscribe to live events first, then replay. Live events that
            # overlap this response are filtered by their run sequence number.
            if replay_run_records:
                response = await self._event_client.get(
                    "/run/records", params={"after": self._run_after}, timeout=10.0
                )
                response.raise_for_status()
                records = response.json()
                if not isinstance(records, list):
                    raise ValueError("Invalid run record replay")
                for record in records:
                    if not isinstance(record, dict):
                        raise ValueError("Invalid run record")
                    await self._handle_message(record)
            for item in event.data["items"]:
                if not isinstance(item, dict):
                    raise ValueError("Invalid station event snapshot item")
                await self._handle_message(item)
        elif event.kind == "station":
            if not isinstance(event.data, dict):
                raise ValueError("Invalid station event")
            await self._handle_message(event.data)
        else:
            return
        async with self._condition:
            self._connected = True
            self._disconnected_at = None
            self._condition.notify_all()

    async def _handle_message(self, message: dict[str, object]) -> None:
        kind = message.get("kind")
        if not isinstance(kind, str):
            log.debug("Ignoring station message without a kind")
            return
        try:
            update = _STATION_EVENT_MESSAGE.validate_python(message)
        except ValidationError as error:
            match error.errors(
                include_url=False, include_context=False, include_input=False
            ):
                case [{"type": "union_tag_invalid", "loc": ()}]:
                    log.debug("Ignoring station message kind %r", kind)
                    return
                case _:
                    raise

        match update:
            case ArmFrame(arm=arm, frame=frame):
                async with self._condition:
                    self._keep_frame(arm, frame)
                    self._condition.notify_all()
            case StateMessage(state=state):
                async with self._condition:
                    self._state = state
                    self._state_count += 1
                    self._condition.notify_all()
            case ActionSequenceState(action_sequence=action_sequence):
                async with self._condition:
                    self._action_sequence = action_sequence
                    self._action_sequence_revision += 1
                    self._condition.notify_all()
            case RunEventRecord() | RunSpanRecord() as record:
                self._deliver_run_record(record)
            case RunResync(newest=newest) as resync:
                self._run_after = newest
                self._publish_run(resync)
            case RunMessage() as run:
                self._publish_run(run)
            case _ as unreachable:
                assert_never(unreachable)

    def _publish_run(self, update: RunUpdate) -> None:
        for queue in self._run_subscribers:
            queue.put_nowait(update)

    def _deliver_run_record(self, record: RunEventRecord | RunSpanRecord) -> None:
        """Publish a run record once, in order, after a resume or a duplicate."""
        if self._run_after is not None and record.seq <= self._run_after:
            return
        self._run_after = record.seq
        self._publish_run(record)

    def _keep_frame(self, arm: str, frame: Frame) -> None:
        """Keep command records and advance the arm's resume cursor."""
        cursor = frame.seq
        previous = self._last_action_sequence.get(arm)
        if cursor is not None and previous is not None and cursor <= previous:
            return

        match frame.event:
            case Record() as event:
                self._keep_record(
                    ArmRecord(
                        arm=arm,
                        record=CommandRecord(
                            accepted_at=event.accepted_at,
                            checkpoints=event.checkpoints,
                            command=event.command,
                            command_id=event.command_id,
                            label=event.label,
                            phase=event.phase,
                            trace_context=event.trace_context,
                        ),
                    )
                )
            case SnapshotTaken(log=retained_frames, records=records, seq=sequence):
                for retained in retained_frames:
                    self._keep_frame(arm, retained)
                for record in records:
                    self._keep_record(ArmRecord(arm=arm, record=record))
                cursor = sequence
            case (
                State()
                | Reply()
                | Resync()
                | Mode()
                | Rejected()
                | Tripped()
                | Event1()
                | Event2()
            ):
                pass
            case _ as unreachable:
                assert_never(unreachable)
        if cursor is not None:
            self._last_action_sequence[arm] = cursor

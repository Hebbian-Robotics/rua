"""Async station commands and leases over HTTP."""

import asyncio
import logging
import os
import time
import uuid
from collections.abc import AsyncGenerator, Awaitable, Callable
from contextlib import asynccontextmanager
from types import TracebackType
from typing import Self

from pydantic import TypeAdapter

from hebbian_rua.bindings import _native
from hebbian_rua.client.contract.arm_commands import (
    ArmMotionCommands,
    ArmPowerCommands,
)
from hebbian_rua.client.contract.station_api import (
    ActionSequenceGripper,
    ActionSequenceMove,
    ActionSequenceRecord,
    ActionSequenceRequest,
    ActionSequenceStep,
    ArmRecord,
    ArmState,
    CameraListing,
    CommandAccepted,
    CommandRecord,
    CommandRefused,
    CompilationArtifact,
    CompilationOutcome,
    GraspOrientation,
    LeaseHolder,
    LeaseStatus,
    MoveArmsCommand,
    ObservationAnswer,
    ObservationRequest,
    OnInterruption,
    OperationStatus,
    PhoneInvite,
    PosePreviewBatchRequest,
    PosePreviewBatchResponse,
    PosePreviewCandidate,
    PoseProjection,
    PreviewPose,
    PreviewProjected,
    PreviewRefused,
    ProgramCompiled,
    ProgramContinuation,
    ProgramRejected,
    QuaternionOrientation,
    RecordingStatus,
    RejectionReason,
    RunEventRecord,
    RunMessage,
    RunResync,
    RunSpanRecord,
    StationState,
    TraceFile,
)
from hebbian_rua.client.contract.trace_spans import TraceSpan
from hebbian_rua.client.station_types import (
    DEFAULT_SERVER,
    ActionSequenceUnavailable,
    CameraJpeg,
    CameraUnavailable,
    CommandResult,
    LeaseHeldElsewhere,
    LeaseLost,
    StationCommand,
    trace_context_for_commands,
)
from hebbian_rua.client.station_types import (
    JointSpeedRadiansPerSecond as JointSpeedRadiansPerSecond,
)
from hebbian_rua.client.station_types import (
    ResultOutcome as ResultOutcome,
)
from hebbian_rua.client.station_types import (
    WorldPositionMeters as WorldPositionMeters,
)
from hebbian_rua.client.station_types import (
    command_record_was_limited as command_record_was_limited,
)
from hebbian_rua.client.station_types import (
    commands_parented_to as commands_parented_to,
)

log = logging.getLogger(__name__)


type RunUpdate = RunEventRecord | RunSpanRecord | RunResync | RunMessage
_run_update = TypeAdapter(RunUpdate)
_arm_records = TypeAdapter(list[ArmRecord])


def _command_options(
    expected_revisions: dict[str, int], operation_id: uuid.UUID | None = None
) -> dict[str, object]:
    trace = trace_context_for_commands.get()
    return {
        "expected_revisions": expected_revisions,
        "trace_context": trace.model_dump(mode="json") if trace else None,
        "operation_id": str(operation_id) if operation_id else None,
    }


async def _lease_operation(operation: Awaitable[None]) -> None:
    try:
        await operation
    except _native.CommandRefused as error:
        if error.reason == RejectionReason.lease_held_elsewhere:
            raise LeaseLost from error
        raise


def _command_refusal(error: _native.CommandRefused) -> CommandRefused:
    return CommandRefused(reason=RejectionReason(error.reason), detail=error.detail)


async def _command_result(result: Awaitable[object]) -> CommandResult:
    try:
        record = await result
    except _native.CommandRefused as error:
        return CommandResult(_command_refusal(error))
    except _native.CommandIncomplete as error:
        record = error.record
    return CommandResult(CommandRecord.model_validate(record))


class Arm(ArmPowerCommands):
    """Observations and the commands that need no lease for one station arm:
    power (`enable`, `disable`), fault recovery (`clear_fault`,
    `approve_recovery`) and `stop`. Motion is on `Lease.arm`."""

    def __init__(self, client: "StationClient", name: str) -> None:
        self._client = client
        self.name = name

    async def state(self) -> ArmState:
        station = await self._client.state()
        for arm in station.arms:
            if arm.arm == self.name:
                return arm
        raise LookupError(f"The station has no arm {self.name}")

    async def _run_command(
        self,
        command: StationCommand,
        *,
        expected_revision: int | None = None,
    ) -> CommandResult:
        options = _command_options(
            {self.name: expected_revision} if expected_revision is not None else {}
        )
        return await _command_result(
            self._client._native.run_command(
                self.name, command.model_dump(mode="json"), options
            )
        )

    async def preview_pose(
        self,
        position: list[float],
        orientation: GraspOrientation | QuaternionOrientation | None = None,
        *,
        max_joint_speed: float | None = None,
        max_joint_acceleration: float | None = None,
        max_joint_jerk: float | None = None,
    ) -> PoseProjection | PreviewRefused:
        return await self._client.preview_pose(
            self.name,
            PreviewPose(
                position=position,
                orientation=orientation,
                max_joint_speed=max_joint_speed,
                max_joint_acceleration=max_joint_acceleration,
                max_joint_jerk=max_joint_jerk,
            ),
        )

    async def preview_poses(
        self, poses: list[PosePreviewCandidate]
    ) -> PosePreviewBatchResponse:
        """Preview independent candidates and return answers in input order."""
        return await self._client.preview_poses(self.name, poses)


class LeasedArm(ArmMotionCommands):
    """The commands that move one arm, which exist only under the lease."""

    def __init__(self, lease: "Lease", name: str) -> None:
        self._lease = lease
        self.name = name

    async def _run_command(
        self, command: StationCommand, *, expected_revision: int | None = None
    ) -> CommandResult:
        options = _command_options(
            {self.name: expected_revision} if expected_revision is not None else {}
        )
        return await _command_result(
            self._lease._guard.run_command(
                self.name, command.model_dump(mode="json"), options
            )
        )

    async def run_action_sequence(
        self,
        steps: list[ActionSequenceMove | ActionSequenceGripper],
        *,
        expected_revision: int | None = None,
    ) -> ActionSequenceRecord:
        """Execute ordered steps and return their final state.

        The station refuses the action_sequence while an interlock is active, stops it
        once one is raised, and stops it if this connection disconnects.
        """
        action_sequence_id = await self._lease.start_action_sequence(
            ActionSequenceRequest(
                steps=[
                    ActionSequenceStep(arms={self.name: action}) for action in steps
                ],
                expected_revisions={self.name: expected_revision}
                if expected_revision is not None
                else {},
            )
        )
        return await self._lease._client.wait_for_action_sequence(action_sequence_id)

    async def move_to_pose(
        self,
        position: list[float],
        orientation: GraspOrientation | QuaternionOrientation | None = None,
        *,
        max_joint_speed: float | None = None,
        max_joint_acceleration: float | None = None,
        max_joint_jerk: float | None = None,
        expected_revision: int | None = None,
    ) -> CommandResult:
        """As the generated command, but `None` keeps the tool's current orientation."""
        if orientation is None:
            tool = (await self._lease._client.arm(self.name).state()).tool
            if tool is None:
                raise ValueError(f"{self.name} has no pose yet; is it enabled?")
            orientation = QuaternionOrientation(quaternion=tool.quaternion)
        return await super().move_to_pose(
            position,
            orientation,
            max_joint_speed=max_joint_speed,
            max_joint_acceleration=max_joint_acceleration,
            max_joint_jerk=max_joint_jerk,
            expected_revision=expected_revision,
        )


class Lease:
    """The lease on the arms, held by one connection and yielded by
    `StationClient.holding_lease`. Everything that moves an arm is here; the
    lease token stays inside."""

    def __init__(self, client: "StationClient", guard: _native.NativeLease) -> None:
        self._client = client
        self._guard = guard
        self.status = LeaseStatus.model_validate(guard.status())

    def arm(self, name: str) -> LeasedArm:
        return LeasedArm(self, name)

    async def move_arms(
        self,
        command: MoveArmsCommand,
        *,
        expected_revisions: dict[str, int] | None = None,
        operation_id: uuid.UUID | None = None,
    ) -> list[CommandResult]:
        """Move several arms together, planned before any moves."""
        try:
            records = await self._guard.move_arms(
                command.model_dump(mode="json"),
                _command_options(expected_revisions or {}, operation_id),
            )
        except _native.CommandRefused as error:
            return [CommandResult(_command_refusal(error))]
        return [
            CommandResult(entry.record)
            for entry in _arm_records.validate_python(records)
        ]

    async def start_action_sequence(
        self, action_sequence: ActionSequenceRequest
    ) -> str:
        """Submit an ordered action_sequence and return its station-assigned identifier;
        follow it with `StationClient.wait_for_action_sequence`."""
        return await self._guard.start_action_sequence(
            action_sequence.model_dump(mode="json")
        )

    async def start_compiled_sequence(
        self,
        compilation_id: str,
        *,
        expected_revisions: dict[str, int] | None = None,
        continuation: ProgramContinuation | None = None,
    ) -> str:
        """Submit a retained compilation under this lease; return its execution ID."""
        return await self._guard.start_compiled_sequence(
            compilation_id,
            expected_revisions or {},
            continuation.model_dump(mode="json") if continuation else None,
        )

    async def send(
        self,
        arm: str,
        command: StationCommand,
        *,
        expected_revision: int | None = None,
        operation_id: uuid.UUID | None = None,
    ) -> CommandAccepted | CommandRefused:
        """Submit one arm's command and return the station's answer without
        waiting for its outcome."""
        try:
            records = await self._guard.send_operation(
                arm,
                str(operation_id or uuid.uuid4()),
                command.model_dump(mode="json"),
                expected_revision,
            )
        except _native.CommandRefused as error:
            return _command_refusal(error)
        return CommandAccepted(records=_arm_records.validate_python(records))

    async def update_stream_goal(
        self,
        operation_id: uuid.UUID,
        sequence: int,
        joints: list[float],
        *,
        gripper_opening: float | None = None,
        expected_revision: int | None = None,
    ) -> CommandAccepted | CommandRefused:
        """Send the next goal to a running stream, using an increasing sequence."""
        try:
            records = await self._guard.update_stream_goal(
                str(operation_id), sequence, joints, gripper_opening, expected_revision
            )
        except _native.CommandRefused as error:
            return _command_refusal(error)
        return CommandAccepted(records=_arm_records.validate_python(records))

    async def cancel_operation(self, operation_id: uuid.UUID) -> None:
        await self._guard.cancel_operation(str(operation_id))

    async def report_camera(self, camera: str, available: bool) -> None:
        await _lease_operation(self._guard.report_camera(camera, available))

    async def stop_arm(self, arm: str) -> None:
        await _lease_operation(self._guard.stop_arm(arm))

    async def stop_recording(self) -> None:
        await self._guard.stop_recording()


class StationClient:
    """Station command client whose lifetime belongs to the calling event loop."""

    def __init__(
        self,
        server: str | None = None,
        *,
        media_server: str | None = None,
        run_after: int | None = None,
    ) -> None:
        """Use the station URL for cameras unless a media URL is passed or set in HEBBIAN_MEDIA_SERVER.

        `run_after` resumes `run_telemetry` after that sequence number.
        """
        if server is None:
            server = os.environ.get("HEBBIAN_STATION_URL") or DEFAULT_SERVER
        if media_server is None:
            media_server = os.environ.get("HEBBIAN_MEDIA_SERVER") or server
        self._native = _native.NativeStation(server, media_server, run_after)
        self._guard: _native.NativeLease | None = None
        self._closed = False
        self.lease_lost = asyncio.Event()

    async def __aenter__(self) -> Self:
        return self

    async def __aexit__(
        self,
        exception_type: type[BaseException] | None,
        exception: BaseException | None,
        traceback: TracebackType | None,
    ) -> None:
        await self.close()

    async def close(self) -> None:
        self._closed = True
        try:
            if self._guard is not None:
                await self._guard.release()
        finally:
            self.lease_lost.set()
            self._native.close()

    async def connect(self) -> None:
        await self._native.wait_until_connected()

    async def state(self) -> StationState:
        return StationState.model_validate(await self._native.state())

    async def wait_for_state(
        self, predicate: Callable[[StationState], bool]
    ) -> StationState:
        async with asyncio.timeout(30):
            while True:
                cursor = self.change_stamp()
                state = await self.state()
                if predicate(state):
                    return state
                await self.wait_for_change(cursor)

    async def run_telemetry(self) -> AsyncGenerator[RunUpdate]:
        telemetry = self._native.telemetry()
        while (message := await telemetry.receive()) is not None:
            yield _run_update.validate_python(message)

    def change_stamp(self) -> tuple[int, int]:
        return self._native.observation_cursor()

    async def wait_for_change(self, observed: tuple[int, int]) -> None:
        await self._native.wait_for_observation(observed)

    def arm(self, name: str) -> Arm:
        return Arm(self, name)

    async def resolve_arm_name(self, name: str | None = None) -> str:
        """Select a named arm, or the station's sole arm when no name is given."""
        arm_names = [arm.arm for arm in (await self.state()).arms]
        if name is not None and name in arm_names:
            return name
        if name is None and len(arm_names) == 1:
            return arm_names[0]
        if not arm_names:
            raise ValueError("The station reports no arms")
        available = ", ".join(arm_names)
        if name is None:
            raise ValueError(f"Select an arm from: {available}")
        raise ValueError(f"Unknown arm {name!r}. Available arms: {available}")

    async def camera_names(self) -> list[str]:
        return await self._native.camera_names()

    async def capture_observation(
        self, selection: ObservationRequest
    ) -> ObservationAnswer:
        """Capture selected arm state and camera frames through the station."""
        return ObservationAnswer.model_validate(
            await self._native.capture_observation_http(
                selection.model_dump(mode="json")
            )
        )

    async def get_operation(self, operation_id: uuid.UUID) -> OperationStatus:
        """Look up a motion operation using the ID that submitted it."""
        return OperationStatus.model_validate(
            await self._native.get_operation(str(operation_id))
        )

    async def wait_for_command(self, command_id: str) -> CommandRecord:
        """Wait for an admitted command's outcome and the corresponding arm state."""
        return CommandRecord.model_validate(
            await self._native.wait_for_outcome(command_id)
        )

    async def camera_jpeg(
        self, name: str, *, maximum_bytes: int | None = None
    ) -> CameraJpeg:
        started = time.time()
        try:
            jpeg, captured = await self._native.camera_jpeg(name, maximum_bytes)
        except _native.MissingCamera as error:
            raise CameraUnavailable(
                name,
                TypeAdapter(list[CameraListing]).validate_python(error.listed),
                error.last_error,
            ) from error
        return CameraJpeg(jpeg, (started, time.time()), captured)

    async def start_recording(self) -> str:
        return await self._native.start_recording()

    async def ensure_recording(self) -> str:
        return await self._native.ensure_recording()

    async def recording_status(self) -> RecordingStatus:
        return RecordingStatus.model_validate(await self._native.recording_status())

    async def stop_recording(self) -> None:
        await self._native.stop_recording()

    @asynccontextmanager
    async def recording(self) -> AsyncGenerator[str]:
        name = await self.start_recording()
        try:
            yield name
        finally:
            await self.stop_recording()

    async def record_run_event(self, event: dict[str, object]) -> None:
        try:
            await self._native.record_run_event(event)
        except (ConnectionError, _native.HttpError) as error:
            log.warning("Run telemetry unavailable: %s", error)

    async def record_run_spans(self, spans: list[TraceSpan]) -> None:
        try:
            await self._native.record_run_spans(
                [span.model_dump(mode="json") for span in spans]
            )
        except (ConnectionError, _native.HttpError) as error:
            log.warning("Run telemetry unavailable: %s", error)

    async def preview_pose(
        self, arm: str, pose: PreviewPose
    ) -> PoseProjection | PreviewRefused:
        response = await self.preview_poses(
            arm, [PosePreviewCandidate(id=str(uuid.uuid4()), pose=pose)]
        )
        [result] = response.results
        match result.answer:
            case PreviewProjected(projection=projection):
                return projection
            case PreviewRefused() as refusal:
                return refusal

    async def preview_poses(
        self, arm: str, poses: list[PosePreviewCandidate]
    ) -> PosePreviewBatchResponse:
        """Preview independent candidates and return their answers in input order."""
        return PosePreviewBatchResponse.model_validate(
            await self._native.preview_poses(
                PosePreviewBatchRequest(arm=arm, poses=poses).model_dump(mode="json")
            )
        )

    async def compile_action_sequence(
        self, source: ActionSequenceRequest
    ) -> ProgramCompiled | ProgramRejected:
        """Compile an action sequence without acquiring control or moving hardware."""
        return CompilationOutcome.model_validate(
            await self._native.compile_action_sequence(source.model_dump(mode="json"))
        ).root

    async def import_compilation(
        self, artifact: CompilationArtifact
    ) -> ProgramCompiled | ProgramRejected:
        """Load an export by recompiling its source; saved plans grant no authority."""
        return CompilationOutcome.model_validate(
            await self._native.import_compilation(artifact.model_dump(mode="json"))
        ).root

    async def inspect_compilation(self, compilation_id: str) -> CompilationArtifact:
        """Inspect a retained compilation; missing or expired artifacts return HTTP 404."""
        return CompilationArtifact.model_validate(
            await self._native.inspect_compilation(compilation_id)
        )

    async def action_sequence_updates(
        self,
    ) -> AsyncGenerator[ActionSequenceRecord | None]:
        revision = 0
        while True:
            revision, snapshot = await self._native.next_sequence(revision)
            yield (
                ActionSequenceRecord.model_validate(snapshot)
                if snapshot is not None
                else None
            )

    async def get_action_sequence(
        self, action_sequence_id: str
    ) -> ActionSequenceRecord:
        try:
            return ActionSequenceRecord.model_validate(
                await self._native.get_action_sequence(action_sequence_id)
            )
        except _native.HttpError as error:
            if error.status == 404:
                raise ActionSequenceUnavailable(action_sequence_id) from error
            raise

    async def wait_for_action_sequence(
        self, action_sequence_id: str
    ) -> ActionSequenceRecord:
        try:
            return ActionSequenceRecord.model_validate(
                await self._native.wait_for_action_sequence(action_sequence_id)
            )
        except _native.HttpError as error:
            if error.status == 404:
                raise ActionSequenceUnavailable(action_sequence_id) from error
            raise

    async def cancel_action_sequence(self, action_sequence_id: str) -> None:
        await self._native.cancel_action_sequence(action_sequence_id)

    @asynccontextmanager
    async def holding_lease(
        self,
        label: str,
        *,
        take: bool = False,
        required_cameras: list[str] | None = None,
        on_interruption: OnInterruption = OnInterruption.wait_for_approval,
        holder: LeaseHolder = LeaseHolder.agent,
    ) -> AsyncGenerator[Lease]:
        """Acquire the lease (or with `take`, take it from its holder), yield the
        `Lease` that commands the arms, and return it on exit. Raises
        `LeaseHeldElsewhere` when someone else holds it."""
        request = {
            "label": label,
            "holder": holder.value,
            "required_cameras": required_cameras or [],
            "on_interruption": on_interruption.value,
        }
        try:
            guard = await self._native.acquire_lease(request, take)
        except _native.LeaseHeld as error:
            holder_status = error.holder_status
            raise LeaseHeldElsewhere(
                LeaseStatus.model_validate(holder_status)
                if holder_status is not None
                else None
            ) from error
        if self._closed:
            await guard.release()
            raise ConnectionError("Station closed while acquiring lease")
        self._guard = guard
        self.lease_lost.clear()

        loss = guard.lost()

        async def watch_loss() -> None:
            await loss
            if self._guard is guard:
                self.lease_lost.set()

        watcher = asyncio.create_task(watch_loss())
        try:
            yield Lease(self, guard)
        finally:
            try:
                await guard.release()
            except BaseException:
                watcher.cancel()
                raise
            finally:
                await asyncio.gather(watcher, return_exceptions=True)
                if self._guard is guard:
                    self._guard = None

    async def revoke_lease(self) -> None:
        await self._native.revoke_lease()

    async def phone_invite(self, label: str) -> PhoneInvite:
        return PhoneInvite.model_validate(
            await self._native.phone_invite({"label": label})
        )

    async def list_traces(self) -> list[TraceFile]:
        return TypeAdapter(list[TraceFile]).validate_python(
            await self._native.list_traces()
        )

    async def download_trace(self, name: str) -> bytes:
        return await self._native.download_trace(name)

"""Async station commands and leases over HTTP."""

import asyncio
import logging
import time
import uuid
from collections.abc import AsyncGenerator, Awaitable, Callable
from contextlib import asynccontextmanager
from enum import Enum, auto
from types import TracebackType
from typing import Self

import httpx2

from hebbian_rua.client.contract.arm_commands import (
    ArmMotionCommands,
    ArmPowerCommands,
)
from hebbian_rua.client.contract.camera_api import CAPTURED_AT_HEADER
from hebbian_rua.client.contract.http_routes import StationApi
from hebbian_rua.client.contract.station_api import (
    ActionSequenceArmAction,
    ActionSequenceRecord,
    ActionSequenceRequest,
    ActionSequenceStartRequest,
    ActionSequenceStep,
    ArmState,
    CommandAccepted,
    CommandCancellationRequest,
    CommandRefused,
    CommandSubmissionRequest,
    CompilationArtifact,
    GraspOrientation,
    GroupCommandSubmissionRequest,
    LeaseAcquireRequest,
    LeaseCameraReport,
    LeaseGranted,
    LeaseHolder,
    LeaseRequest,
    LeaseStatus,
    LeaseToken,
    MoveArmsCommand,
    OnInterruption,
    PhoneInvite,
    PhoneInviteRequest,
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
    RunCompiledProgramRequest,
    RunEventMessage,
    RunSpansMessage,
    RunSpansRequest,
    StationState,
    TraceFile,
)
from hebbian_rua.client.contract.trace_spans import TraceSpan
from hebbian_rua.client.station_connection import (
    MessageRefused,
    RunUpdate,
    StationConnection,
)
from hebbian_rua.client.station_types import (
    CAMERA_TIMEOUT_SECONDS,
    DEFAULT_MEDIA_SERVER,
    DEFAULT_SERVER,
    RUNS_TIMEOUT_SECONDS,
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
        return await self._client._run(
            self.name, command, expected_revision=expected_revision
        )

    async def preview_pose(
        self,
        position: list[float],
        orientation: GraspOrientation | QuaternionOrientation | None = None,
    ) -> PoseProjection | PreviewRefused:
        return await self._client.preview_pose(
            self.name, PreviewPose(position=position, orientation=orientation)
        )

    async def preview_poses(
        self, poses: list[PosePreviewCandidate]
    ) -> PosePreviewBatchResponse:
        """Preview independent candidates and return answers in input order."""
        return await self._client.preview_poses(self.name, poses)


class LeasedArm(ArmMotionCommands):
    """The commands that move one arm, which exist only under the lease."""

    def __init__(self, client: "StationClient", name: str, lease_token: str) -> None:
        self._client = client
        self.name = name
        self._lease_token = lease_token

    async def _run_command(
        self,
        command: StationCommand,
        *,
        expected_revision: int | None = None,
    ) -> CommandResult:
        return await self._client._run(
            self.name,
            command,
            expected_revision=expected_revision,
            expected_lease=self._lease_token,
        )

    async def run_action_sequence(
        self,
        steps: list[ActionSequenceArmAction],
        *,
        expected_revision: int | None = None,
    ) -> ActionSequenceRecord:
        """Execute ordered steps and return their final state.

        The station refuses the action_sequence while an interlock is active, stops it
        once one is raised, and stops it if this connection disconnects.
        """
        self._client._check_lease(self._lease_token)
        action_sequence_id = await self._client._start_action_sequence(
            ActionSequenceRequest(
                steps=[
                    ActionSequenceStep(arms={self.name: action}) for action in steps
                ],
                expected_revisions={self.name: expected_revision}
                if expected_revision is not None
                else {},
            )
        )
        return await self._client.wait_for_action_sequence(action_sequence_id)

    async def move_to_pose(
        self,
        position: list[float],
        orientation: GraspOrientation | QuaternionOrientation | None = None,
        *,
        max_joint_speed: float | None = None,
        expected_revision: int | None = None,
    ) -> CommandResult:
        """As the generated command, but `None` keeps the tool's current orientation."""
        if orientation is None:
            tool = (await self._client.arm(self.name).state()).tool
            if tool is None:
                raise ValueError(f"{self.name} has no pose yet; is it enabled?")
            orientation = QuaternionOrientation(quaternion=tool.quaternion)
        return await super().move_to_pose(
            position,
            orientation,
            max_joint_speed=max_joint_speed,
            expected_revision=expected_revision,
        )

    async def rehome(
        self, *, close_gripper: bool = True, expected_revision: int | None = None
    ) -> CommandResult:
        """Return to the enable-time pose, then close the gripper, at up to 2 rad/s.

        Closing starts only after the home move completes and stops on contact.
        Pass `close_gripper=False` to leave the gripper as it is.
        """
        rest_pose = (await self._client.arm(self.name).state()).rest_pose
        if rest_pose is None:
            return CommandResult.of(
                CommandRefused(
                    reason=RejectionReason.not_enabled,
                    detail=f"{self.name} has no rest pose; enable it first",
                )
            )
        rehome_speed = min(2.0, (await self._client.state()).max_joint_speed)
        homed = await self.move_to_joints(
            list(rest_pose),
            max_joint_speed=rehome_speed,
            expected_revision=expected_revision,
        )
        if not homed.completed or not close_gripper:
            return homed
        return await self.set_gripper(
            0.0, max_joint_speed=rehome_speed, expected_revision=expected_revision
        )


class Lease:
    """The lease on the arms, held by one connection and yielded by
    `StationClient.holding_lease`. Everything that moves an arm is here; the
    lease token stays inside."""

    def __init__(self, client: "StationClient", grant: LeaseGranted) -> None:
        self._client = client
        self.grant = grant

    @property
    def status(self) -> LeaseStatus:
        return self.grant.status

    def arm(self, name: str) -> LeasedArm:
        """The commands that move one arm and its gripper."""
        return LeasedArm(self._client, name, self.grant.lease_token)

    async def move_arms(
        self,
        command: MoveArmsCommand,
        *,
        expected_revisions: dict[str, int] | None = None,
    ) -> list[CommandResult]:
        """Move several arms together, planned before any moves."""
        return await self._client._run_group(
            command,
            expected_revisions=expected_revisions,
            expected_lease=self.grant.lease_token,
        )

    async def start_action_sequence(
        self, action_sequence: ActionSequenceRequest
    ) -> str:
        """Submit an ordered action_sequence and return its station-assigned identifier;
        follow it with `StationClient.wait_for_action_sequence`."""
        self._client._check_lease(self.grant.lease_token)
        return await self._client._start_action_sequence(action_sequence)

    async def start_compiled_sequence(
        self,
        compilation_id: str,
        *,
        expected_revisions: dict[str, int] | None = None,
        continuation: ProgramContinuation | None = None,
    ) -> str:
        """Submit a retained compilation under this lease; return its execution ID."""
        self._client._check_lease(self.grant.lease_token)
        return await self._client._start_compiled_sequence(
            compilation_id,
            expected_revisions=expected_revisions,
            continuation=continuation,
        )

    async def send(
        self,
        arm: str,
        command: StationCommand,
        *,
        expected_revision: int | None = None,
    ) -> CommandAccepted | CommandRefused:
        """Submit one arm's command and return the station's answer without
        waiting for its outcome."""
        try:
            return await self._client._send(
                arm,
                command,
                expected_revision=expected_revision,
                expected_lease=self.grant.lease_token,
            )
        except LeaseLost:
            return CommandRefused(
                reason=RejectionReason.lease_held_elsewhere,
                detail="The lease is no longer held",
            )

    async def report_camera(self, camera: str, available: bool) -> None:
        """Report whether a camera the lease requires gives frames; a fresh frame
        clears its interlock."""
        self._client._check_lease(self.grant.lease_token)
        await self._client._report_camera(camera, available)

    async def stop_arm(self, arm: str) -> None:
        """Request a stop only while this lease is still held."""
        self._client._check_lease(self.grant.lease_token)
        await self._client._stop_leased_arm(arm)

    async def stop_recording(self) -> None:
        """Stop recording only while this lease is still held."""
        self._client._check_lease(self.grant.lease_token)
        await self._client._stop_recording_for_lease()


class _LeasePhase(Enum):
    IDLE = auto()
    ACQUIRING = auto()


class StationClient:
    """Station command client whose lifetime belongs to the calling event loop."""

    def __init__(
        self,
        server: str = DEFAULT_SERVER,
        *,
        media_server: str = DEFAULT_MEDIA_SERVER,
        run_after: int | None = None,
    ) -> None:
        """`run_after` resumes `run_telemetry` after that sequence number."""
        self._connection = StationConnection(server, run_after=run_after)
        self._lease_state: _LeasePhase | LeaseGranted = _LeasePhase.IDLE
        self._lease_renewal: asyncio.Task[None] | None = None
        self._station_http = httpx2.AsyncClient(
            base_url=server, timeout=RUNS_TIMEOUT_SECONDS
        )
        self._api = StationApi(
            self._station_http,
            httpx2.AsyncClient(base_url=media_server, timeout=CAMERA_TIMEOUT_SECONDS),
        )

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
        try:
            await self._close_lease()
        finally:
            try:
                await self._connection.close()
            finally:
                await self._api.close()

    async def _close_lease(self) -> None:
        if self._lease_renewal is not None:
            self._lease_renewal.cancel()
            await asyncio.gather(self._lease_renewal, return_exceptions=True)
            self._lease_renewal = None
        if isinstance(self._lease_state, LeaseGranted):
            token = self._lease_state.lease_token
            self._lease_state = _LeasePhase.IDLE
            try:
                await self._api.release_lease_http(LeaseToken(lease_token=token))
            except httpx2.HTTPError as error:
                log.warning("Could not release lease during close: %s", error)

    @property
    def lease_lost(self) -> asyncio.Event:
        return self._connection.lease_lost

    async def connect(self) -> None:
        """Wait for the station connection, which other calls otherwise open on
        first use. A refused connection fails at once with `ConnectionError`."""
        await self._connection.connect()

    async def state(self) -> StationState:
        return await self._connection.state()

    async def wait_for_state(
        self, predicate: Callable[[StationState], bool]
    ) -> StationState:
        """Wait for a state frame that satisfies `predicate` and return it; the
        latest frame is checked first. Raises `TimeoutError` after 30 s."""
        return await self._connection.wait_for_state(predicate)

    async def run_telemetry(self) -> AsyncGenerator[RunUpdate]:
        """A model loop's run telemetry from the first message on: events and
        spans in order, each once, a resync when the cursor fell out of the
        retained records, and the live run snapshot."""
        async for update in self._connection.run_telemetry():
            yield update

    def change_stamp(self) -> tuple[object, ...]:
        """Identify each arm's mode, revision and interlocks now; see `wait_for_change`."""
        return self._connection.change_stamp()

    async def wait_for_change(self, observed: tuple[object, ...]) -> None:
        """Wait until an arm's mode, revision or interlocks, or the connection,
        differs from `observed`."""
        await self._connection.wait_for_change(observed)

    def arm(self, name: str) -> Arm:
        return Arm(self, name)

    async def camera_names(self) -> list[str]:
        return [
            camera.name for camera in await self._api.list_cameras() if camera.available
        ]

    async def camera_jpeg(
        self, name: str, *, maximum_bytes: int | None = None
    ) -> CameraJpeg:
        started = time.time()
        try:
            response = await self._api.get_camera(name, maximum_bytes=maximum_bytes)
        except httpx2.HTTPStatusError as error:
            if error.response.status_code != 404:
                raise
            try:
                listed = await self._api.list_cameras()
            except httpx2.HTTPError:
                listed = []
            raise CameraUnavailable(
                name, listed, await self._camera_problem(name)
            ) from error
        captured_at = response.headers.get(CAPTURED_AT_HEADER)
        return CameraJpeg(
            jpeg=response.content,
            retrieved_between_unix_seconds=(started, time.time()),
            captured_at_unix_ns=None if captured_at is None else int(captured_at),
        )

    async def _camera_problem(self, name: str) -> str | None:
        """The latest failure the station saw in the media programs' output for a
        camera, or None when it has none or cannot be asked."""
        try:
            response = await self._station_http.get("/viewer/api/media-status")
            response.raise_for_status()
            detail = response.json()["cameras"][name]["detail"]
        except (httpx2.HTTPError, KeyError, ValueError):
            return None
        return detail if isinstance(detail, str) else None

    async def start_recording(self) -> str:
        return await self._api.start_recording()

    async def ensure_recording(self) -> str:
        """Start recording, or join the active recording across a handover."""
        status = (await self._api.recording_status()).root
        if status.state == "recording":
            return status.name
        return await self.start_recording()

    async def recording_status(self) -> RecordingStatus:
        return await self._api.recording_status()

    async def stop_recording(self) -> None:
        await self._api.stop_recording()

    async def _stop_recording_for_lease(self) -> None:
        """Stop recording only if this client still holds the lease."""
        await self._api.stop_recording_for_lease(
            LeaseToken(lease_token=self._require_lease_token())
        )

    async def _stop_leased_arm(self, arm: str) -> None:
        """Request a stop only while this client still holds the lease."""
        try:
            await self._api.stop_leased_arm_http(
                arm, LeaseToken(lease_token=self._require_lease_token())
            )
        except httpx2.HTTPStatusError as error:
            if error.response.status_code == 403:
                raise LeaseLost from error
            raise

    @property
    def _lease_token(self) -> str | None:
        return (
            self._lease_state.lease_token
            if isinstance(self._lease_state, LeaseGranted)
            else None
        )

    def _require_lease_token(self) -> str:
        if self._lease_token is None:
            raise RuntimeError("This requires the lease")
        if self.lease_lost.is_set():
            raise LeaseLost
        return self._lease_token

    def _check_lease(self, expected_lease: str) -> None:
        if self._lease_token != expected_lease or self.lease_lost.is_set():
            raise LeaseLost

    async def _report_camera(self, camera: str, available: bool) -> None:
        """Report whether a camera the lease requires gives frames; a fresh frame
        clears its interlock."""
        try:
            await self._api.report_lease_camera_http(
                LeaseCameraReport(
                    lease_token=self._require_lease_token(),
                    camera=camera,
                    available=available,
                )
            )
        except httpx2.HTTPStatusError as error:
            if error.response.status_code == 403:
                raise LeaseLost from error
            if error.response.status_code == 409:
                raise MessageRefused(error.response.text) from error
            raise

    @asynccontextmanager
    async def recording(self) -> AsyncGenerator[str]:
        name = await self.start_recording()
        try:
            yield name
        finally:
            await self.stop_recording()

    async def _send_best_effort(
        self, message: RunEventMessage | RunSpansMessage
    ) -> None:
        try:
            if isinstance(message, RunEventMessage):
                await self._api.record_run_event_http(message.event)
            else:
                await self._api.record_run_spans_http(
                    RunSpansRequest(spans=message.spans)
                )
        except (ConnectionError, OSError, httpx2.HTTPError) as error:
            log.warning("Run telemetry unavailable: %s", error)

    async def record_run_event(self, event: dict[str, object]) -> None:
        await self._send_best_effort(RunEventMessage(event=event))

    async def record_run_spans(self, spans: list[TraceSpan]) -> None:
        await self._send_best_effort(
            RunSpansMessage.model_validate(
                {
                    "spans": [
                        span.model_dump(mode="json", by_alias=False) for span in spans
                    ]
                }
            )
        )

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
        return await self._api.preview_poses(
            PosePreviewBatchRequest(arm=arm, poses=poses)
        )

    async def compile_action_sequence(
        self, source: ActionSequenceRequest
    ) -> ProgramCompiled | ProgramRejected:
        """Compile an action sequence without acquiring control or moving hardware."""
        return await self._api.compile_action_sequence(source)

    async def import_compilation(
        self, artifact: CompilationArtifact
    ) -> ProgramCompiled | ProgramRejected:
        """Load an export by recompiling its source; saved plans grant no authority."""
        return await self._api.import_compilation(artifact)

    async def inspect_compilation(self, compilation_id: str) -> CompilationArtifact:
        """Inspect a retained compilation; missing or expired artifacts return HTTP 404."""
        return await self._api.inspect_compilation(compilation_id)

    async def _start_compiled_sequence(
        self,
        compilation_id: str,
        *,
        expected_revisions: dict[str, int] | None = None,
        continuation: ProgramContinuation | None = None,
    ) -> str:
        return await self._api.run_compiled_sequence_http(
            RunCompiledProgramRequest(
                request_id=str(uuid.uuid4()),
                station_instance_id=(await self._api.get_station_instance()).id,
                compilation_id=compilation_id,
                continuation=continuation,
                lease_token=self._require_lease_token(),
                expected_revisions=expected_revisions or {},
            )
        )

    async def _start_action_sequence(
        self, action_sequence: ActionSequenceRequest
    ) -> str:
        """Submit an ordered action_sequence and return its station-assigned identifier."""
        return await self._api.start_action_sequence_http(
            ActionSequenceStartRequest(
                request_id=str(uuid.uuid4()),
                station_instance_id=(await self._api.get_station_instance()).id,
                lease_token=self._require_lease_token(),
                action_sequence=action_sequence,
            )
        )

    async def action_sequence_updates(
        self,
    ) -> AsyncGenerator[ActionSequenceRecord | None]:
        """Yield the latest action_sequence state, including reconnect snapshots."""
        async for action_sequence in self._connection.action_sequence_updates():
            yield action_sequence

    async def get_action_sequence(
        self, action_sequence_id: str
    ) -> ActionSequenceRecord:
        """Get a running or recently completed action_sequence by identifier."""
        return await self._api.get_action_sequence(action_sequence_id)

    async def wait_for_action_sequence(
        self, action_sequence_id: str
    ) -> ActionSequenceRecord:
        """Wait until the specified action_sequence ends."""
        async for action_sequence in self.action_sequence_updates():
            if (
                action_sequence is not None
                and action_sequence.id == action_sequence_id
                and action_sequence.phase.state != "running"
            ):
                return action_sequence
            if action_sequence is not None and action_sequence.id != action_sequence_id:
                previous = await self.get_action_sequence(action_sequence_id)
                if previous.phase.state != "running":
                    return previous
        raise ConnectionError("ActionSequence updates ended")

    async def cancel_action_sequence(self, action_sequence_id: str) -> None:
        """Ask the station to cancel an accepted action_sequence."""
        try:
            await self._api.cancel_action_sequence_http(action_sequence_id)
        except httpx2.HTTPStatusError as error:
            if error.response.status_code != 404:
                raise

    async def _send(
        self,
        arm: str,
        command: StationCommand,
        *,
        expected_revision: int | None = None,
        expected_lease: str | None = None,
    ) -> CommandAccepted | CommandRefused:
        if expected_lease is not None:
            self._check_lease(expected_lease)
        if command.kind not in {
            "enable",
            "disable",
            "stop",
            "clear_fault",
            "approve_recovery",
        }:
            lease_token = self._require_lease_token()
        else:
            lease_token = self._lease_token
        instance = await self._api.get_station_instance()
        request = CommandSubmissionRequest(
            arm=arm,
            command=command,
            request_id=str(uuid.uuid4()),
            station_instance_id=instance.id,
            lease_token=lease_token,
            trace_context=trace_context_for_commands.get(),
            expected_revision=expected_revision,
        )
        return await self._submit_http_command(
            lambda: self._api.submit_command_http(request),
            request_id=request.request_id,
            station_instance_id=request.station_instance_id,
            lease_token=request.lease_token,
            arms=[arm],
        )

    async def _submit_http_command(
        self,
        submit: Callable[[], Awaitable[CommandAccepted | CommandRefused]],
        *,
        request_id: str,
        station_instance_id: str,
        lease_token: str | None,
        arms: list[str],
    ) -> CommandAccepted | CommandRefused:
        """Retry the same submission; fence it if its caller stops waiting."""
        try:
            async with asyncio.timeout(30):
                while True:
                    try:
                        answer = await submit()
                        break
                    except httpx2.TransportError:
                        await asyncio.sleep(0.1)
        except (asyncio.CancelledError, TimeoutError):
            if lease_token is not None and arms:
                cancellation = CommandCancellationRequest(
                    request_id=request_id,
                    station_instance_id=station_instance_id,
                    lease_token=lease_token,
                    arms=arms,
                )
                try:
                    await asyncio.shield(
                        self._api.cancel_command_submission_http(cancellation)
                    )
                except (httpx2.HTTPError, OSError) as error:
                    log.warning("Could not cancel command %s: %s", request_id, error)
            raise
        if isinstance(answer, CommandAccepted):
            for arm_record in answer.records:
                await self._connection.remember(arm_record)
        return answer

    async def _run(
        self,
        arm: str,
        command: StationCommand,
        *,
        expected_revision: int | None = None,
        expected_lease: str | None = None,
    ) -> CommandResult:
        captured_lease = expected_lease or self._lease_token
        try:
            answer = await self._send(
                arm,
                command,
                expected_revision=expected_revision,
                expected_lease=expected_lease,
            )
            if isinstance(answer, CommandRefused):
                return CommandResult.of(answer)
            [record] = answer.records
            return CommandResult.of(
                await self._connection.wait_for_outcome(record.record.command_id)
            )
        except asyncio.CancelledError:
            await self._stop_after_cancellation(arm, captured_lease)
            raise

    async def _stop_after_cancellation(self, arm: str, lease_token: str | None) -> None:
        if lease_token is None:
            return
        try:
            await asyncio.shield(
                self._api.stop_leased_arm_http(arm, LeaseToken(lease_token=lease_token))
            )
        except (
            MessageRefused,
            ConnectionError,
            OSError,
            TimeoutError,
            httpx2.HTTPError,
        ) as error:
            log.warning("Could not stop %s after cancellation: %s", arm, error)

    async def _send_group(
        self,
        command: MoveArmsCommand,
        *,
        expected_revisions: dict[str, int] | None = None,
        expected_lease: str | None = None,
    ) -> CommandAccepted | CommandRefused:
        if expected_lease is not None:
            self._check_lease(expected_lease)
        lease_token = self._require_lease_token()
        instance = await self._api.get_station_instance()
        request = GroupCommandSubmissionRequest(
            command=command,
            request_id=str(uuid.uuid4()),
            station_instance_id=instance.id,
            lease_token=lease_token,
            trace_context=trace_context_for_commands.get(),
            expected_revisions=expected_revisions or {},
        )
        return await self._submit_http_command(
            lambda: self._api.submit_group_command_http(request),
            request_id=request.request_id,
            station_instance_id=request.station_instance_id,
            lease_token=request.lease_token,
            arms=list(command.targets),
        )

    async def _run_group(
        self,
        command: MoveArmsCommand,
        *,
        expected_revisions: dict[str, int] | None = None,
        expected_lease: str | None = None,
    ) -> list[CommandResult]:
        captured_lease = expected_lease or self._lease_token
        try:
            answer = await self._send_group(
                command,
                expected_revisions=expected_revisions,
                expected_lease=expected_lease,
            )
            if isinstance(answer, CommandRefused):
                return [CommandResult.of(answer)]
            return [
                CommandResult.of(
                    await self._connection.wait_for_outcome(record.record.command_id)
                )
                for record in answer.records
            ]
        except asyncio.CancelledError:
            await asyncio.gather(
                *(
                    self._stop_after_cancellation(arm, captured_lease)
                    for arm in command.targets
                )
            )
            raise

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
        if self._lease_state is not _LeasePhase.IDLE:
            raise RuntimeError("This client already holds or is acquiring a lease")
        self._lease_state = _LeasePhase.ACQUIRING
        try:
            async with self._holding_lease(
                label,
                take=take,
                required_cameras=required_cameras,
                on_interruption=on_interruption,
                holder=holder,
            ) as lease:
                yield lease
        finally:
            if self._lease_state is _LeasePhase.ACQUIRING:
                self._lease_state = _LeasePhase.IDLE

    @asynccontextmanager
    async def _holding_lease(
        self,
        label: str,
        *,
        take: bool,
        required_cameras: list[str] | None,
        on_interruption: OnInterruption,
        holder: LeaseHolder,
    ) -> AsyncGenerator[Lease]:
        await self._connection.connect()
        self.lease_lost.clear()
        instance = await self._api.get_station_instance()
        request = LeaseAcquireRequest(
            request_id=str(uuid.uuid4()),
            station_instance_id=instance.id,
            take=take,
            request=LeaseRequest(
                holder=holder,
                label=label,
                required_cameras=required_cameras or [],
                on_interruption=on_interruption,
            ),
        )
        try:
            async with asyncio.timeout(30):
                while True:
                    try:
                        grant_response = await self._api.acquire_lease_http(request)
                        break
                    except httpx2.TransportError:
                        await asyncio.sleep(0.1)
        except httpx2.HTTPStatusError as error:
            if error.response.status_code == 409 and error.response.text.startswith(
                "The lease is held by "
            ):
                try:
                    current_holder = (await self._api.get_state()).lease
                except (httpx2.HTTPError, OSError):
                    current_holder = None
                raise LeaseHeldElsewhere(current_holder) from error
            raise
        grant = LeaseGranted(
            lease_token=grant_response.lease_token,
            lease_seconds=grant_response.lease_seconds,
            status=grant_response.status,
        )
        try:
            await self._api.renew_lease_http(LeaseToken(lease_token=grant.lease_token))
        except httpx2.HTTPStatusError as error:
            if error.response.status_code == 409:
                raise LeaseLost from error
            raise
        if self.lease_lost.is_set():
            try:
                await asyncio.shield(
                    self._api.release_lease_http(
                        LeaseToken(lease_token=grant.lease_token)
                    )
                )
            except (httpx2.HTTPError, OSError) as error:
                log.warning(
                    "Could not release lease after losing the event stream: %s", error
                )
            raise ConnectionError(
                "The station event stream was lost while acquiring the lease"
            )
        self._lease_state = grant
        self._lease_renewal = asyncio.create_task(self._renew_lease(grant))
        try:
            yield Lease(self, grant)
        finally:
            renewal = self._lease_renewal
            self._lease_renewal = None
            if renewal is not None:
                renewal.cancel()
                await asyncio.gather(renewal, return_exceptions=True)
            if self._lease_state is grant:
                self._lease_state = _LeasePhase.IDLE
                try:
                    await self._api.release_lease_http(
                        LeaseToken(lease_token=grant.lease_token)
                    )
                except httpx2.HTTPStatusError as error:
                    if error.response.status_code != 409:
                        raise

    async def _renew_lease(self, grant: LeaseGranted) -> None:
        """Renew while the station's event stream remains usable."""
        interval = max(0.1, grant.lease_seconds / 3)
        try:
            while True:
                await asyncio.sleep(interval)
                if self.lease_lost.is_set():
                    return
                async with asyncio.timeout(interval):
                    await self._connection.state()
                    if self.lease_lost.is_set():
                        return
                    await self._api.renew_lease_http(
                        LeaseToken(lease_token=grant.lease_token)
                    )
        except (httpx2.HTTPError, OSError, TimeoutError, ConnectionError) as error:
            log.warning("Lease renewal stopped: %s", error)
            self.lease_lost.set()

    async def revoke_lease(self) -> None:
        """The operator's override: end whoever holds the lease.

        Returns once the station reports that no one holds it, so acquiring it
        right after succeeds.
        """
        async with self.holding_lease("operator override", take=True):
            # The cached frame can predate the take and show no holder, so only
            # a frame received after the take's reply counts.
            frames_before_return = self._connection.state_count
        await self._connection.wait_for_state(
            lambda state: state.lease is None,
            after_frame_count=frames_before_return,
        )

    async def phone_invite(self, label: str) -> PhoneInvite:
        """An invite to stream a phone as camera `phone_<label>`."""
        return await self._api.phone_invite(PhoneInviteRequest(label=label))

    async def list_traces(self) -> list[TraceFile]:
        """The executor's trace files, newest first."""
        return await self._api.list_traces()

    async def download_trace(self, name: str) -> bytes:
        """A trace file's bytes exactly as served: gzip-compressed."""
        return (await self._api.get_trace_file(name)).content

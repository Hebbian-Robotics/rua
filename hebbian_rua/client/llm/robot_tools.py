"""Async model-facing robot tools."""

import asyncio
from base64 import b64decode
from collections.abc import Awaitable, Callable, Mapping, Sequence
from copy import copy, deepcopy
from dataclasses import dataclass, field, replace
from typing import Self

from pydantic import TypeAdapter

from hebbian_rua.bindings import _native
from hebbian_rua.client.contract.station_api import (
    ObservationAnswer1,
    ObservationRequest,
    StationState,
)
from hebbian_rua.client.llm.response_format import (
    default_formatter,
    result_from_data,
)
from hebbian_rua.client.llm.tool_catalog import (
    RobotToolConfiguration,
    ToolTransformation,
    all_robot_tool_capabilities,
    capabilities_from_configuration,
    configuration_from_capabilities,
    create_robot_tool_catalog,
    default_robot_tool_mapping,
    native_configuration,
)
from hebbian_rua.client.llm.types import (
    OutputFormatter,
    RobotObservation,
    RobotOutput,
    RobotToolCall,
    RobotToolResult,
    RobotToolSpec,
    SequenceImageHistory,
)
from hebbian_rua.client.station_client import (
    CameraJpeg,
    Lease,
    StationClient,
    _command_options,
)

type CallWrapper = Callable[
    [RobotToolCall, Callable[[], Awaitable[RobotToolResult]]],
    Awaitable[RobotToolResult],
]


class RequiredCameraUnavailable(Exception):
    def __init__(self, camera: str) -> None:
        self.camera = camera
        super().__init__(f"Required camera {camera} is unavailable")


class ArmRecoveryEnded(Exception):
    """The arm was disabled or faulted before it recovered."""


class ObservationWaitTimedOut(Exception):
    """The station reached the deadline before the observation was ready."""


class ArmCommunicationInterrupted(Exception):
    """An arm observation is unavailable until communication recovers."""


@dataclass(frozen=True)
class CapturedObservation:
    observation: RobotObservation
    captures: dict[str, CameraJpeg] = field(default_factory=dict)


_history = TypeAdapter(SequenceImageHistory | None)


class RobotTools:
    """Model tools backed by the async station client."""

    def __init__(
        self,
        station: StationClient,
        lease: Lease,
        arm: str,
        cameras: Sequence[str] = (),
        *,
        optional_cameras: Sequence[str] = (),
        descriptions: Mapping[str, str] | None = None,
        configuration: RobotToolConfiguration | None = None,
    ) -> None:
        names = [*cameras, *optional_cameras]
        if len(set(names)) != len(names):
            raise ValueError(f"Camera names repeat: {names}")
        self._output_formatter: OutputFormatter = default_formatter
        self.station = station
        self.lease = lease
        self.arm = arm
        self.cameras = list(cameras)
        self.optional_cameras = list(optional_cameras)
        self.descriptions = dict(descriptions or {})
        self.configuration = (
            configuration
            if configuration is not None
            else configuration_from_capabilities(
                default_robot_tool_mapping(all_robot_tool_capabilities())
            )
        )
        self._catalog = create_robot_tool_catalog(self.configuration)
        self._native = _native.NativeTools(
            lease._guard, arm, self.cameras, self.optional_cameras
        ).configured(native_configuration(self.configuration))

    def map_tools(self, transform: ToolTransformation) -> Self:
        """Return a capability view sharing this connection, lease and output formatter."""
        capabilities = tuple(
            transform(capabilities_from_configuration(self.configuration))
        )
        configuration = configuration_from_capabilities(capabilities)
        catalog = create_robot_tool_catalog(configuration)
        mapped = copy(self)
        mapped.configuration = configuration
        mapped._catalog = catalog
        mapped._native = self._native.configured(native_configuration(configuration))
        return mapped

    def with_output_formatter(self, formatter: OutputFormatter) -> Self:
        """Return a view using this formatter on full data; share the connection.

        The formatter replaces the previous formatter. It does not alter
        execution, leases or fault/interruption flags. Close the shared station
        only when all views are finished with it.
        """
        mapped = copy(self)
        mapped._output_formatter = formatter
        return mapped

    async def context(self) -> dict[str, object]:
        """Return factual conventions and the current station's robot description."""
        return await self._native.context()

    @property
    def tools(self) -> list[RobotToolSpec]:
        return [
            RobotToolSpec(
                name=tool.name,
                description=self.descriptions.get(tool.name, tool.description),
                parameters_schema=deepcopy(tool.parameters_schema),
            )
            for tool in self._catalog
        ]

    def sequence_images(self) -> SequenceImageHistory | None:
        """Selected images from the latest sequence, available for five minutes."""
        return _history.validate_python(self._native.history())

    async def observe(self, note: str = "") -> RobotObservation:
        return (await self.capture(note)).observation

    async def capture(
        self,
        note: str = "",
        extra_lines: Sequence[str] = (),
        *,
        wait_milliseconds: int | None = None,
    ) -> CapturedObservation:
        output, captures, revisions = await self._capture_output(
            note, extra_lines, wait_milliseconds=wait_milliseconds
        )
        content = self._output_formatter(deepcopy(output))
        return CapturedObservation(
            RobotObservation(
                content.text, content.cameras, data=output.data, revisions=revisions
            ),
            captures,
        )

    async def _capture_output(
        self,
        note: str = "",
        extra_lines: Sequence[str] = (),
        *,
        wait_milliseconds: int | None = None,
    ) -> tuple[RobotOutput, dict[str, CameraJpeg], dict[str, int]]:
        selection = ObservationRequest(
            required_cameras=list(self.cameras),
            optional_cameras=list(self.optional_cameras),
        )
        answer = (
            await self.station.capture_observation(selection)
            if wait_milliseconds is None
            else await self.lease.wait_for_observation(
                selection, self.arm, wait_milliseconds
            )
        )
        capture = answer.root
        if not isinstance(capture, ObservationAnswer1):
            if capture.status == "observation_required_camera_unavailable":
                raise RequiredCameraUnavailable(capture.camera)
            if capture.status == "observation_wait_timed_out":
                raise ObservationWaitTimedOut
            if capture.status == "observation_recovery_ended":
                raise ArmRecoveryEnded
            raise ArmCommunicationInterrupted(
                f"Arm {capture.arm} changed or disconnected during observation"
            )
        state = capture.state
        if not any(arm.arm == self.arm for arm in state.arms):
            raise ValueError(f"The station has no arm {self.arm}")
        captures = {
            name: CameraJpeg(
                jpeg=b64decode(frame.jpeg_base64, validate=True),
                retrieved_between_unix_seconds=(
                    frame.retrieved_between_unix_seconds[0],
                    frame.retrieved_between_unix_seconds[1],
                ),
                captured_at_unix_ns=int(frame.captured_at_unix_ns)
                if frame.captured_at_unix_ns
                else None,
            )
            for name, frame in capture.cameras.items()
        }
        raw_data = state.model_dump(mode="json")
        output = RobotOutput(
            "observation",
            raw_data,
            cameras={name: capture.jpeg for name, capture in captures.items()},
            selected_arm=self.arm,
            notes=(*extra_lines, *([note] if note else [])),
            missing_optional_cameras=tuple(capture.missing_optional_cameras),
            camera_intervals={
                name: capture.retrieved_between_unix_seconds
                for name, capture in captures.items()
            },
        )
        return output, captures, {arm.arm: arm.revision for arm in state.arms}

    async def execute(
        self,
        calls: Sequence[RobotToolCall],
        wrap: CallWrapper | None = None,
        *,
        expected_revision: int | None = None,
        expected_revisions: Mapping[str, int] | None = None,
    ) -> list[RobotToolResult]:
        """Execute model calls concurrently and return results in input order."""
        revisions = dict(expected_revisions or {})
        if expected_revision is not None:
            if self.arm in revisions and revisions[self.arm] != expected_revision:
                raise ValueError("Conflicting revisions for the selected arm")
            revisions[self.arm] = expected_revision

        async def execute_call(call: RobotToolCall) -> RobotToolResult:
            started = False

            async def run(current_call: RobotToolCall = call) -> RobotToolResult:
                nonlocal started
                if started:
                    raise RuntimeError("A tool wrapper may run its call only once")
                started = True
                outcome = await self._native.execute(
                    current_call.call_id,
                    current_call.name,
                    current_call.arguments,
                    _command_options(revisions),
                )
                history = _history.validate_python(outcome["sequence_images"])
                data = outcome["data"]
                observed_revisions = None
                if (
                    current_call.name == "observe"
                    and isinstance(data, dict)
                    and "arms" in data
                ):
                    state = StationState.model_validate(data)
                    observed_revisions = {arm.arm: arm.revision for arm in state.arms}
                return result_from_data(
                    current_call,
                    data,
                    disposition=outcome["disposition"],
                    cameras=outcome["cameras"] if history is None else None,
                    sequence_images=history,
                    observed_revisions=observed_revisions,
                )

            return await wrap(call, run) if wrap is not None else await run()

        outcomes = await asyncio.gather(
            *(execute_call(call) for call in calls), return_exceptions=True
        )
        results: list[RobotToolResult] = []
        for call, outcome in zip(calls, outcomes, strict=True):
            if isinstance(outcome, asyncio.CancelledError):
                results.append(
                    result_from_data(
                        call,
                        "Tool call was cancelled; its outcome may be unknown",
                        disposition="interrupted",
                    )
                )
            elif isinstance(outcome, Exception):
                results.append(
                    result_from_data(
                        call,
                        f"Tool call failed; its outcome may be unknown: {outcome}",
                        disposition="interrupted",
                    )
                )
            elif isinstance(outcome, BaseException):
                raise outcome
            else:
                results.append(outcome)
        if self._output_formatter is default_formatter:
            return results
        formatted_results = []
        for result in results:
            raw_data = result.data
            content = self._output_formatter(
                RobotOutput(
                    "result",
                    deepcopy(raw_data),
                    call=result.call,
                    cameras=deepcopy(result.cameras)
                    if result.sequence_images is None
                    else {},
                    sequence_images=deepcopy(result.sequence_images),
                )
            )
            formatted_results.append(
                replace(result, text=content.text, cameras=content.cameras)
            )
        return formatted_results

    async def execute_one(
        self,
        call: RobotToolCall,
        *,
        expected_revision: int | None = None,
        expected_revisions: Mapping[str, int] | None = None,
    ) -> RobotToolResult:
        return (
            await self.execute(
                [call],
                expected_revision=expected_revision,
                expected_revisions=expected_revisions,
            )
        )[0]

"""Async model-facing robot tools."""

import asyncio
from base64 import b64decode
from collections.abc import Awaitable, Callable, Mapping, Sequence
from copy import copy, deepcopy
from dataclasses import replace
from math import degrees
from typing import Self
from uuid import uuid4

from hebbian_rua.client.contract.station_api import (
    ActionSequenceRequest,
    ArmMode,
    CommandOutcome,
    ObservationAnswer1,
    ObservationRequest,
    PosePreviewCandidate,
)
from hebbian_rua.client.llm.generated_robot_tools import ROBOT_TOOL_CONTEXT
from hebbian_rua.client.llm.response_format import (
    default_formatter,
    model_response,
    result_from_data,
)
from hebbian_rua.client.llm.robot_tool_rules import (
    CapturedObservation,
    RobotToolRules,
    command_results,
)
from hebbian_rua.client.llm.robot_tool_rules import (
    records_result as records_result,
)
from hebbian_rua.client.llm.sequence_images import (
    SequenceImages,
    capture_sequence_images,
)
from hebbian_rua.client.llm.tool_catalog import (
    RobotToolConfiguration,
    ToolTransformation,
    all_robot_tool_capabilities,
    capabilities_from_configuration,
    configuration_from_capabilities,
    create_robot_tool_catalog,
    default_robot_tool_mapping,
)
from hebbian_rua.client.llm.tools import (
    ActionSequenceInput,
    ArmGripperInput,
    MoveArmsInput,
    ObserveInput,
    PreviewPoseInput,
    RehomeInput,
    RobotToolInput,
    StopInput,
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
)
from hebbian_rua.client.station_connection import MessageRefused

type CallWrapper = Callable[
    [RobotToolCall, Callable[[], Awaitable[RobotToolResult]]],
    Awaitable[RobotToolResult],
]


class RequiredCameraUnavailable(Exception):
    def __init__(self, camera: str) -> None:
        self.camera = camera
        super().__init__(f"Required camera {camera} is unavailable")


class ArmCommunicationInterrupted(Exception):
    """An arm observation is unavailable until communication recovers."""


class RobotTools(RobotToolRules):
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
        self._sequence_images = SequenceImages()
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
        state = await self.station.state()
        return {
            "conventions": dict(ROBOT_TOOL_CONTEXT),
            "profile": state.robot,
            "description": state.description,
            "joint_names": list(state.joint_names),
            "arms": [
                {"name": arm.arm, "has_gripper": arm.gripper_opening is not None}
                for arm in state.arms
            ],
            "max_joint_speed_degrees_per_second": degrees(state.max_joint_speed),
            "jaw_closing_axis": state.jaw_closing_axis.value,
            "surface_guard": model_response(state.surface_guard.model_dump(mode="json"))
            if state.surface_guard is not None
            else None,
            "model_result_units": "Fields ending in _mm use millimeters and fields ending in _degrees use degrees; speeds use degrees per second in both inputs and outputs.",
        }

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
        return self._sequence_images.snapshot()

    async def observe(self, note: str = "") -> RobotObservation:
        return (await self.capture(note)).observation

    async def capture(
        self, note: str = "", extra_lines: Sequence[str] = ()
    ) -> CapturedObservation:
        output, captures, revisions = await self._capture_output(note, extra_lines)
        content = self._output_formatter(deepcopy(output))
        return CapturedObservation(
            RobotObservation(
                content.text, content.cameras, data=output.data, revisions=revisions
            ),
            captures,
        )

    async def _capture_output(
        self, note: str = "", extra_lines: Sequence[str] = ()
    ) -> tuple[RobotOutput, dict[str, CameraJpeg], dict[str, int]]:
        capture = (
            await self.station.capture_observation(
                ObservationRequest(
                    required_cameras=list(self.cameras),
                    optional_cameras=list(self.optional_cameras),
                )
            )
        ).root
        if not isinstance(capture, ObservationAnswer1):
            if capture.status == "observation_required_camera_unavailable":
                raise RequiredCameraUnavailable(capture.camera)
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
                parsed = self.parse(current_call)
                if isinstance(parsed, RobotToolResult):
                    return parsed
                try:
                    return await self._execute_individual(
                        current_call, parsed, revisions
                    )
                except MessageRefused as refusal:
                    return result_from_data(current_call, refusal.detail, is_error=True)

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
                        is_error=True,
                        interrupted=True,
                    )
                )
            elif isinstance(outcome, Exception):
                results.append(
                    result_from_data(
                        call,
                        f"Tool call failed; its outcome may be unknown: {outcome}",
                        is_error=True,
                        interrupted=True,
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

    async def _execute_individual(
        self,
        call: RobotToolCall,
        tool_input: RobotToolInput,
        expected_revisions: Mapping[str, int],
    ) -> RobotToolResult:
        expected_revision = expected_revisions.get(self.arm)
        match tool_input:
            case PreviewPoseInput(poses=poses):
                candidates = [
                    PosePreviewCandidate(id=str(uuid4()), pose=candidate.pose)
                    for candidate in poses
                ]
                response = await self.station.preview_poses(self.arm, candidates)
                answers_by_id = {
                    candidate.id: candidate.answer for candidate in response.results
                }
                return result_from_data(
                    call,
                    {
                        "results": [
                            {
                                "answer": answers_by_id[candidate.id].model_dump(
                                    mode="json"
                                )
                            }
                            for candidate in candidates
                        ]
                    },
                    is_error=False,
                )
            case ObserveInput():
                return await self._observe_tool(call, tool_input)
            case MoveArmsInput():
                records = await self.lease.move_arms(
                    tool_input,
                    expected_revisions=dict(expected_revisions),
                )
                return command_results(call, records, grouped=True)
            case ArmGripperInput(arm=arm, opening=opening, max_joint_speed=speed):
                record = await self.lease.arm(arm).set_gripper(
                    opening,
                    max_joint_speed=speed,
                    expected_revision=expected_revisions.get(arm),
                )
                return command_results(call, [record], grouped=False)
            case StopInput():
                record = await self.station._run(self.arm, tool_input)
                return command_results(call, [record], grouped=False)
            case RehomeInput():
                record = await self.lease.arm(self.arm).rehome(
                    expected_revision=expected_revision
                )
                return command_results(call, [record], grouped=False)
            case ActionSequenceInput(steps=steps):
                async with capture_sequence_images(
                    self._sequence_images,
                    call.call_id,
                    [*self.cameras, *self.optional_cameras],
                    self.station,
                ) as set_sequence_id:
                    sequence_id = await self.lease.start_action_sequence(
                        ActionSequenceRequest(
                            steps=steps,
                            expected_revisions=dict(expected_revisions),
                        )
                    )
                    set_sequence_id(sequence_id)
                    return await self._action_sequence_result(call, sequence_id)

    async def _observe_tool(
        self, call: RobotToolCall, request: ObserveInput
    ) -> RobotToolResult:
        await asyncio.sleep(request.wait_seconds)
        if request.source == "last_sequence":
            history = self.sequence_images()
            if history is None:
                return result_from_data(
                    call,
                    "No sequence image history is available; none was captured or it expired.",
                    is_error=True,
                )
            data = {
                "source": "last_sequence",
                "sequence_call_id": history.call_id,
                "missed_samples": history.missed_samples,
                "truncated": history.truncated,
                "images": [
                    {
                        "camera": image.camera,
                        "elapsed_seconds": image.elapsed_seconds,
                        "retrieved_between_unix_seconds": image.retrieved_between_unix_seconds,
                        "boundary": image.boundary,
                    }
                    for image in history.images
                ],
            }
            return result_from_data(call, data, is_error=False, sequence_images=history)
        output, _captures, revisions = await self._capture_output()
        return result_from_data(
            call,
            output.data,
            is_error=False,
            cameras=output.cameras,
            observed_revisions=revisions,
        )

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

    async def _action_sequence_result(
        self, call: RobotToolCall, action_sequence_id: str
    ) -> RobotToolResult:
        action_sequence = await self.station.wait_for_action_sequence(
            action_sequence_id
        )
        outcomes = {
            record.record.completion.outcome
            for step in action_sequence.steps
            for record in step
            if record.record.completion is not None
        }
        modes = {
            arm.mode
            for arm in (await self.station.state()).arms
            if arm.arm in action_sequence.arms
        }
        return result_from_data(
            call,
            action_sequence.model_dump(mode="json"),
            is_error=action_sequence.phase.state != "completed",
            halt=CommandOutcome.faulted in outcomes or ArmMode.faulted in modes,
            interrupted=CommandOutcome.interrupted in outcomes
            or bool(modes & {ArmMode.disconnected, ArmMode.reconnected}),
        )

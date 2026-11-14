"""Async model-facing robot tools."""

import asyncio
import json
from collections.abc import Awaitable, Callable, Mapping, Sequence
from copy import deepcopy

import httpx2

from hebbian_robot_use_client.contract.station_api import (
    ActionSequenceRequest,
    ArmMode,
    CommandOutcome,
)
from hebbian_robot_use_client.llm.generated_robot_tools import ROBOT_TOOL_CONTEXT
from hebbian_robot_use_client.llm.response_format import model_response
from hebbian_robot_use_client.llm.robot_tool_rules import (
    ARM_SUMMARIES_ADAPTER,
    CapturedObservation,
    RobotToolRules,
    arm_summary,
    command_results,
)
from hebbian_robot_use_client.llm.robot_tool_rules import (
    records_result as records_result,
)
from hebbian_robot_use_client.llm.tools import (
    ROBOT_TOOLS,
    ActionSequenceInput,
    ArmGripperInput,
    MoveArmsInput,
    ObserveInput,
    PreviewPoseInput,
    RehomeInput,
    RobotToolInput,
    StopInput,
)
from hebbian_robot_use_client.llm.types import (
    RobotObservation,
    RobotToolCall,
    RobotToolResult,
    RobotToolSpec,
)
from hebbian_robot_use_client.station_client import (
    CameraJpeg,
    Lease,
    StationClient,
)
from hebbian_robot_use_client.station_connection import MessageRefused

type CallWrapper = Callable[
    [RobotToolCall, Callable[[], Awaitable[RobotToolResult]]],
    Awaitable[RobotToolResult],
]

OPTIONAL_CAMERA_TIMEOUT_SECONDS = 0.5


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
    ) -> None:
        names = [*cameras, *optional_cameras]
        if len(set(names)) != len(names):
            raise ValueError(f"Camera names repeat: {names}")
        self.station = station
        self.lease = lease
        self.arm = arm
        self.cameras = list(cameras)
        self.optional_cameras = list(optional_cameras)
        self.descriptions = dict(descriptions or {})

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
            "max_joint_speed_radians_per_second": state.max_joint_speed,
            "jaw_closing_axis": state.jaw_closing_axis.value,
            "surface_guard": state.surface_guard.model_dump(mode="json")
            if state.surface_guard is not None
            else None,
            "model_result_units": "Fields ending in _mm use millimeters and fields ending in _degrees use degrees; inputs use meters and radians.",
        }

    @property
    def tools(self) -> list[RobotToolSpec]:
        return [
            RobotToolSpec(
                name=tool.name,
                description=self.descriptions.get(tool.name, tool.description),
                parameters_schema=deepcopy(tool.parameters_schema),
            )
            for tool in ROBOT_TOOLS
        ]

    async def observe(self, note: str = "") -> RobotObservation:
        return (await self.capture(note)).observation

    async def capture(
        self, note: str = "", extra_lines: Sequence[str] = ()
    ) -> CapturedObservation:
        state = await self.station.state()
        arms = {arm.arm: arm_summary(state, arm) for arm in state.arms}
        if self.arm not in arms:
            raise ValueError(f"The station has no arm {self.arm}")
        if arms[self.arm].mode in {ArmMode.disconnected, ArmMode.reconnected}:
            raise ArmCommunicationInterrupted(f"Arm {self.arm} is disconnected")

        async def required_camera(name: str) -> CameraJpeg:
            try:
                return await self.station.camera_jpeg(name)
            except (httpx2.HTTPError, OSError) as error:
                raise RequiredCameraUnavailable(name) from error

        async def optional_camera(name: str) -> CameraJpeg:
            async with asyncio.timeout(OPTIONAL_CAMERA_TIMEOUT_SECONDS):
                return await self.station.camera_jpeg(name)

        # Optional snapshots run alongside the required ones, so a slow one
        # adds nothing beyond its own bound to the critical path.
        optional_snapshots = asyncio.gather(
            *(optional_camera(name) for name in self.optional_cameras),
            return_exceptions=True,
        )
        try:
            required_results = await asyncio.gather(
                *(required_camera(name) for name in self.cameras)
            )
        except BaseException:
            optional_snapshots.cancel()
            raise
        captures = dict(zip(self.cameras, required_results, strict=True))
        optional_results = await optional_snapshots
        missing_optional = []
        for name, result in zip(self.optional_cameras, optional_results, strict=True):
            if isinstance(result, Exception):
                missing_optional.append(name)
            elif isinstance(result, BaseException):
                raise result
            else:
                captures[name] = result
        lines = [
            *extra_lines,
            *([note] if note else []),
            f"Robot profile: {state.robot or 'unknown'}.",
            f"Arm state: {json.dumps(model_response(arms[self.arm].model_dump(mode='json')))}",
            "All arm states: "
            + json.dumps(
                model_response(ARM_SUMMARIES_ADAPTER.dump_python(arms, mode="json"))
            ),
        ]
        if missing_optional:
            lines.append("Optional cameras unavailable: " + ", ".join(missing_optional))
        if captures:
            lines.append(
                "Camera retrieval intervals (Unix seconds): "
                + json.dumps(
                    {
                        name: [
                            round(timestamp, 3)
                            for timestamp in capture.retrieved_between_unix_seconds
                        ]
                        for name, capture in captures.items()
                    }
                )
            )
        return CapturedObservation(
            RobotObservation(
                text="\n".join(lines),
                cameras={name: capture.jpeg for name, capture in captures.items()},
            ),
            captures,
        )

    async def execute(
        self,
        calls: Sequence[RobotToolCall],
        wrap: CallWrapper | None = None,
        *,
        expected_revision: int | None = None,
    ) -> list[RobotToolResult]:
        """Execute model calls concurrently and return results in input order."""

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
                        current_call, parsed, expected_revision
                    )
                except MessageRefused as refusal:
                    return RobotToolResult(current_call, refusal.detail, is_error=True)

            return await wrap(call, run) if wrap is not None else await run()

        outcomes = await asyncio.gather(
            *(execute_call(call) for call in calls), return_exceptions=True
        )
        results: list[RobotToolResult] = []
        for call, outcome in zip(calls, outcomes, strict=True):
            if isinstance(outcome, asyncio.CancelledError):
                results.append(
                    RobotToolResult(
                        call,
                        "Tool call was cancelled; its outcome may be unknown",
                        is_error=True,
                        interrupted=True,
                    )
                )
            elif isinstance(outcome, Exception):
                results.append(
                    RobotToolResult(
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
        return results

    async def _execute_individual(
        self,
        call: RobotToolCall,
        tool_input: RobotToolInput,
        expected_revision: int | None,
    ) -> RobotToolResult:
        match tool_input:
            case PreviewPoseInput(poses=poses):
                response = await self.station.preview_poses(self.arm, poses)
                return RobotToolResult(
                    call,
                    json.dumps(model_response(response.model_dump(mode="json"))),
                    False,
                )
            case ObserveInput(wait_seconds=wait_seconds):
                await asyncio.sleep(wait_seconds)
                state = await self.station.state()
                return RobotToolResult(
                    call,
                    json.dumps(model_response(state.model_dump(mode="json"))),
                    False,
                )
            case MoveArmsInput():
                records = await self.lease.move_arms(
                    tool_input,
                    expected_revisions={}
                    if expected_revision is None
                    else {self.arm: expected_revision},
                )
                return command_results(call, records, grouped=True)
            case ArmGripperInput(arm=arm, opening=opening, max_joint_speed=speed):
                record = await self.lease.arm(arm).set_gripper(
                    opening,
                    max_joint_speed=speed,
                    expected_revision=expected_revision if arm == self.arm else None,
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
                sequence_id = await self.lease.start_action_sequence(
                    ActionSequenceRequest(
                        arm=self.arm,
                        steps=steps,
                        expected_revision=expected_revision,
                    )
                )
                return await self._action_sequence_result(call, sequence_id)

    async def execute_one(
        self, call: RobotToolCall, *, expected_revision: int | None = None
    ) -> RobotToolResult:
        return (await self.execute([call], expected_revision=expected_revision))[0]

    async def _action_sequence_result(
        self, call: RobotToolCall, action_sequence_id: str
    ) -> RobotToolResult:
        action_sequence = await self.station.wait_for_action_sequence(
            action_sequence_id
        )
        outcomes = {
            record.completion.outcome
            for step in action_sequence.steps
            for record in step
            if record.completion is not None
        }
        arm = next(
            (arm for arm in (await self.station.state()).arms if arm.arm == self.arm),
            None,
        )
        return RobotToolResult(
            call,
            json.dumps(model_response(action_sequence.model_dump(mode="json"))),
            is_error=action_sequence.phase.state != "completed",
            halt=CommandOutcome.faulted in outcomes
            or (arm is not None and arm.mode is ArmMode.faulted),
            interrupted=CommandOutcome.interrupted in outcomes
            or (
                arm is not None
                and arm.mode in {ArmMode.disconnected, ArmMode.reconnected}
            ),
        )

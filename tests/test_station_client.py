import asyncio
import json
import math
from pathlib import Path
from uuid import UUID, uuid4

import pytest

from hebbian_rua.client.contract.station_api import (
    ActionSequenceJointsMovement,
    ActionSequenceMove,
    ActionSequenceRequest,
    ActionSequenceStep,
    CommandAccepted,
    CommandRefused,
    EnableCommand,
    LeaseHolder,
    MoveToJointsCommand,
    ObservationRequest,
    PosePreviewCandidate,
    PreviewPose,
    ProgramCompiled,
    ProgramContinuation,
    RecordedGripper,
    RecordedMove,
    RejectionReason,
)
from hebbian_rua.client.llm.robot_tools import RobotTools
from hebbian_rua.client.llm.tool_catalog import RobotToolConfiguration
from hebbian_rua.client.llm.types import RobotToolCall
from hebbian_rua.client.station_client import LeaseHeldElsewhere, StationClient
from hebbian_rua.client.station_types import LeaseLost, command_record_outcome
from tests.fixture_station import (
    ARM,
    running_station,
)


async def arm_interlocks(client: StationClient) -> set[str]:
    return {interlock.kind for interlock in (await client.arm(ARM).state()).interlocks}


@pytest.mark.server
def test_model_can_return_to_the_enable_time_pose(server_binary: Path) -> None:
    with running_station(server_binary) as station:

        async def exercise() -> None:
            async with (
                StationClient(station.server) as client,
                client.holding_lease("rest tool test") as lease,
            ):
                original = (await client.arm(ARM).state()).rest_pose
                assert original is not None
                displaced = list(original)
                displaced[0] += 0.15
                assert (await lease.arm(ARM).move_to_joints(displaced)).completed
                assert (await lease.arm(ARM).set_gripper(1.0)).completed
                tools = RobotTools(
                    client,
                    lease,
                    ARM,
                    configuration=RobotToolConfiguration(
                        tools=(
                            "action_sequence",
                            "preview_pose",
                            "move_arms",
                            "set_arm_gripper",
                            "rehome",
                            "stop",
                            "observe",
                        )
                    ),
                )
                revision = (await client.arm(ARM).state()).revision
                outcome = await tools.execute_one(
                    RobotToolCall("rehome", "rehome", {}), expected_revision=revision
                )
                assert not outcome.is_error, outcome.text
                assert isinstance(outcome.data, dict)
                assert outcome.data["label"] == "rehome"
                assert outcome.data["phase"]["achieved"]["joints"] == pytest.approx(
                    original, abs=0.02
                )
                assert (await client.arm(ARM).state()).gripper_opening == pytest.approx(
                    1.0, abs=0.02
                )
                assert (await client.arm(ARM).state()).measured_joints == pytest.approx(
                    original, abs=0.02
                )

        asyncio.run(asyncio.wait_for(exercise(), timeout=30))


@pytest.mark.server
def test_action_sequence_results_remain_available_after_a_new_action_sequence(
    server_binary: Path,
) -> None:
    with running_station(server_binary) as station:

        async def exercise() -> None:
            async with (
                StationClient(station.server) as client,
                client.holding_lease("action_sequence test") as lease,
            ):
                joints = list((await client.arm(ARM).state()).measured_joints)
                request = ActionSequenceRequest(
                    steps=[
                        ActionSequenceStep(
                            arms={
                                ARM: ActionSequenceMove(
                                    goal=ActionSequenceJointsMovement(joints=joints)
                                )
                            }
                        )
                    ],
                )
                first = await lease.start_action_sequence(request)
                assert (
                    await client.wait_for_action_sequence(first)
                ).phase.state == "completed"
                second = await lease.arm(ARM).run_action_sequence(
                    [step.arms[ARM] for step in request.steps]
                )
                assert second.phase.state == "completed", second.phase
                compiled = await client.compile_action_sequence(request)
                assert isinstance(compiled, ProgramCompiled)
                imported = await client.import_compilation(compiled.artifact)
                assert isinstance(imported, ProgramCompiled)
                explicit_id = await lease.start_compiled_sequence(
                    imported.artifact.compilation_id
                )
                explicit = await client.wait_for_action_sequence(explicit_id)
                assert explicit.compilation_id == imported.artifact.compilation_id
                assert explicit.phase.state == "completed"
                revised = await client.compile_action_sequence(
                    request.model_copy(update={"steps": request.steps * 2})
                )
                assert isinstance(revised, ProgramCompiled)
                continued_id = await lease.start_compiled_sequence(
                    revised.artifact.compilation_id,
                    continuation=ProgramContinuation(
                        prior_execution_id=explicit_id, next_step=1
                    ),
                )
                continued = await client.wait_for_action_sequence(continued_id)
                assert continued.phase.state == "completed"
                assert len(continued.steps) == 1
                assert continued.continuation is not None
                assert continued.continuation.prior_execution_id == explicit_id
                retained = await client.get_action_sequence(first)
                assert retained.phase.state == "completed"
                assert retained.requested_steps == request.steps
                assert (await client.wait_for_action_sequence(first)).id == first

        asyncio.run(asyncio.wait_for(exercise(), timeout=30))


@pytest.mark.server
def test_canceled_sequence_releases_the_arm_for_the_next_sequence(
    server_binary: Path,
) -> None:
    with running_station(server_binary) as station:

        async def exercise() -> None:
            async with (
                StationClient(station.server) as client,
                client.holding_lease("action sequence cancellation") as lease,
            ):
                initial_joints = list((await client.arm(ARM).state()).measured_joints)
                target_joints = initial_joints.copy()
                target_joints[0] += 0.5

                def sequence(joints: list[float]) -> ActionSequenceRequest:
                    return ActionSequenceRequest(
                        steps=[
                            ActionSequenceStep(
                                arms={
                                    ARM: ActionSequenceMove(
                                        goal=ActionSequenceJointsMovement(
                                            joints=joints, max_joint_speed=0.1
                                        )
                                    )
                                }
                            )
                        ]
                    )

                canceled_id = await lease.start_action_sequence(sequence(target_joints))
                await client.cancel_action_sequence(canceled_id)
                canceled = await client.wait_for_action_sequence(canceled_id)
                assert canceled.phase.state == "stopped"
                assert canceled.phase.failure.stage == "cancellation"

                next_id = await lease.start_action_sequence(sequence(initial_joints))
                next_result = await client.wait_for_action_sequence(next_id)
                assert next_result.phase.state == "completed", next_result.phase

        asyncio.run(asyncio.wait_for(exercise(), timeout=15))


@pytest.mark.server
def test_revoking_the_lease_stops_an_action_sequence(server_binary: Path) -> None:
    with running_station(server_binary) as station:

        async def exercise() -> None:
            async with (
                StationClient(station.server) as client,
                StationClient(station.server) as operator,
                client.holding_lease("action_sequence test") as lease,
            ):
                joints = list((await client.arm(ARM).state()).measured_joints)
                joints[0] += 0.5
                request = ActionSequenceRequest(
                    steps=[
                        ActionSequenceStep(
                            arms={
                                ARM: ActionSequenceMove(
                                    goal=ActionSequenceJointsMovement(
                                        joints=joints, max_joint_speed=0.1
                                    )
                                )
                            }
                        ),
                        ActionSequenceStep(
                            arms={
                                ARM: ActionSequenceMove(
                                    goal=ActionSequenceJointsMovement(joints=joints)
                                )
                            }
                        ),
                    ],
                )
                action_sequence_id = await lease.start_action_sequence(request)
                await operator.revoke_lease()
                result = await client.wait_for_action_sequence(action_sequence_id)
                assert result.phase.state == "stopped"
                assert len(result.steps) < 2

        asyncio.run(asyncio.wait_for(exercise(), timeout=30))


@pytest.mark.server
def test_required_camera_loss_stops_a_action_sequence(server_binary: Path) -> None:
    with running_station(server_binary) as station:

        async def exercise() -> None:
            async with (
                StationClient(station.server) as client,
                client.holding_lease(
                    "action_sequence test", required_cameras=["front"]
                ) as lease,
            ):
                await lease.report_camera("front", True)
                joints = list((await client.arm(ARM).state()).measured_joints)
                joints[0] += 0.5
                request = ActionSequenceRequest(
                    steps=[
                        ActionSequenceStep(
                            arms={
                                ARM: ActionSequenceMove(
                                    goal=ActionSequenceJointsMovement(
                                        joints=joints, max_joint_speed=0.1
                                    )
                                )
                            }
                        ),
                        ActionSequenceStep(
                            arms={
                                ARM: ActionSequenceMove(
                                    goal=ActionSequenceJointsMovement(joints=joints)
                                )
                            }
                        ),
                    ],
                )
                action_sequence_id = await lease.start_action_sequence(request)
                await lease.report_camera("front", False)
                result = await client.wait_for_action_sequence(action_sequence_id)
                assert result.phase.state == "stopped"
                assert len(result.steps) < 2

        asyncio.run(asyncio.wait_for(exercise(), timeout=30))


@pytest.mark.server
def test_required_camera_interlock_ends_with_the_lease(server_binary: Path) -> None:
    with running_station(server_binary) as station:

        async def exercise() -> None:
            async with (
                StationClient(station.server) as agent,
                StationClient(station.server) as replacement,
                agent.holding_lease(
                    "agent", required_cameras=["missing_test_camera"]
                ) as agent_lease,
            ):
                arm = agent.arm(ARM)
                await agent_lease.report_camera("missing_test_camera", True)
                await agent.wait_for_state(
                    lambda state: all(
                        interlock.kind != "required_camera_unavailable"
                        for interlock in state.arms[0].interlocks
                    )
                )
                observed_revision = (await arm.state()).revision
                await agent_lease.report_camera("missing_test_camera", False)
                stale_command = await agent_lease.send(
                    ARM, EnableCommand(), expected_revision=observed_revision
                )
                assert isinstance(stale_command, CommandRefused)
                assert stale_command.reason is RejectionReason.stale_revision
                assert (await arm.stop(expected_revision=observed_revision)).completed

                await asyncio.wait_for(
                    agent.wait_for_state(
                        lambda state: any(
                            interlock.kind == "required_camera_unavailable"
                            for interlock in state.arms[0].interlocks
                        )
                    ),
                    timeout=5,
                )
                joints = list((await agent.arm(ARM).state()).measured_joints)
                answer = await agent_lease.send(ARM, MoveToJointsCommand(joints=joints))
                assert isinstance(answer, CommandRefused)
                assert answer.reason is RejectionReason.interlocked
                async with replacement.holding_lease(
                    "replacement", take=True, holder=LeaseHolder.manual
                ) as replacement_lease:
                    assert replacement_lease.status.holder is LeaseHolder.manual
                    await asyncio.wait_for(agent.lease_lost.wait(), timeout=5)
                    with pytest.raises(LeaseLost):
                        await agent_lease.report_camera("missing_test_camera", True)
                    assert not await arm_interlocks(replacement)

        asyncio.run(asyncio.wait_for(exercise(), timeout=20))


@pytest.mark.server
def test_unresponsive_optional_camera_does_not_stall_observation(
    server_binary: Path,
) -> None:
    async def exercise() -> None:
        requested = asyncio.Event()

        async def stalled_camera(
            reader: asyncio.StreamReader, writer: asyncio.StreamWriter
        ) -> None:
            requested.set()
            try:
                await reader.read()
            finally:
                writer.close()
                await writer.wait_closed()

        camera = await asyncio.start_server(stalled_camera, "127.0.0.1", 0)
        async with camera:
            port = camera.sockets[0].getsockname()[1]
            with running_station(
                server_binary, enabled=False, media_address=f"127.0.0.1:{port}"
            ) as station:
                async with (
                    StationClient(station.server) as client,
                    client.holding_lease("camera test") as lease,
                ):
                    assert (await client.arm(ARM).enable()).completed
                    robot = RobotTools(client, lease, ARM, optional_cameras=["front"])
                    observation = await asyncio.wait_for(robot.observe(), timeout=3)
                    assert requested.is_set()
                    assert "Arm state:" in observation.text
                    assert "Optional cameras unavailable: front" in observation.text

    asyncio.run(asyncio.wait_for(exercise(), timeout=10))


@pytest.mark.server
def test_observation_reports_revision_changed_during_camera_capture(
    server_binary: Path,
) -> None:
    async def exercise() -> None:
        camera_requested = asyncio.Event()
        release_frame = asyncio.Event()

        async def media(
            reader: asyncio.StreamReader, writer: asyncio.StreamWriter
        ) -> None:
            try:
                request = await reader.readuntil(b"\r\n\r\n")
                if request.split(b" ")[1] == b"/cameras":
                    mime = b"application/json"
                    body = b'[{"name":"front","available":true}]'
                else:
                    camera_requested.set()
                    await release_frame.wait()
                    mime = b"image/jpeg"
                    body = b"test camera frame"
                writer.write(
                    b"HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Type: "
                    + mime
                    + b"\r\nContent-Length: "
                    + str(len(body)).encode()
                    + b"\r\n\r\n"
                    + body
                )
                await writer.drain()
            finally:
                writer.close()
                await writer.wait_closed()

        camera = await asyncio.start_server(media, "127.0.0.1", 0)
        async with camera:
            port = camera.sockets[0].getsockname()[1]
            with running_station(
                server_binary, enabled=False, media_address=f"127.0.0.1:{port}"
            ) as station:
                async with (
                    StationClient(station.server) as client,
                    client.holding_lease(
                        "revision test", required_cameras=["front"]
                    ) as lease,
                ):
                    assert (await client.arm(ARM).enable()).completed
                    selection = ObservationRequest(
                        arms=[ARM], required_cameras=["front"]
                    )
                    async with asyncio.TaskGroup() as tasks:
                        pending = tasks.create_task(
                            client.capture_observation(selection)
                        )
                        try:
                            await asyncio.wait_for(camera_requested.wait(), timeout=1)
                            await lease.report_camera("front", False)
                        finally:
                            release_frame.set()
                    assert (
                        pending.result().root.status == "observation_revision_changed"
                    )

    asyncio.run(asyncio.wait_for(exercise(), timeout=10))


@pytest.mark.server
def test_taking_the_lease_notifies_the_previous_holder(server_binary: Path) -> None:
    with running_station(server_binary, enabled=False) as station:

        async def exercise_takeover() -> None:
            async with (
                StationClient(station.server) as first,
                StationClient(station.server) as second,
                first.holding_lease("first") as first_lease,
            ):
                with pytest.raises(LeaseHeldElsewhere, match="first"):
                    async with second.holding_lease("second"):
                        pass
                async with second.holding_lease("second", take=True):
                    # WHY: Renewal runs every third of the advertised 15-second lease;
                    # a five-second deadline races the first renewal itself.
                    await asyncio.wait_for(
                        first.lease_lost.wait(),
                        timeout=first_lease.status.expires_in_seconds,
                    )
                    with pytest.raises(LeaseLost):
                        await first_lease.stop_arm(ARM)
                    answer = await first_lease.send(ARM, EnableCommand())
                    assert isinstance(answer, CommandRefused)
                    assert answer.reason == RejectionReason.lease_held_elsewhere
                # INVARIANT: The first connection must not regain authority merely because
                # the newer holder returned its lease.
                await asyncio.wait_for(
                    first.wait_for_state(lambda state: state.lease is None), timeout=5
                )
                answer = await first_lease.send(ARM, EnableCommand())
                assert isinstance(answer, CommandRefused)
                assert answer.reason == RejectionReason.lease_held_elsewhere

            async with StationClient(station.server) as client:

                async def crash_with_held_lease() -> None:
                    async with client.holding_lease("crashing"):
                        await asyncio.wait_for(
                            client.wait_for_state(
                                lambda state: (
                                    state.lease is not None
                                    and state.lease.label == "crashing"
                                )
                            ),
                            timeout=5,
                        )
                        raise RuntimeError("model loop crashed")

                with pytest.raises(RuntimeError, match="model loop crashed"):
                    await crash_with_held_lease()
                await asyncio.wait_for(
                    client.wait_for_state(lambda state: state.lease is None), timeout=5
                )

        asyncio.run(asyncio.wait_for(exercise_takeover(), timeout=20))


@pytest.mark.server
def test_disconnect_returns_the_lease_and_signals_loss(server_binary: Path) -> None:
    with running_station(server_binary, enabled=False) as station:

        async def exercise_disconnect() -> None:
            async with (
                StationClient(station.server) as first,
                StationClient(station.server) as second,
                first.holding_lease("disconnecting"),
            ):
                await first.close()
                assert first.lease_lost.is_set()
                await asyncio.wait_for(
                    second.wait_for_state(lambda state: state.lease is None), timeout=5
                )
                async with second.holding_lease("replacement"):
                    assert (await second.arm(ARM).enable()).completed

        asyncio.run(asyncio.wait_for(exercise_disconnect(), timeout=20))


@pytest.mark.server
def test_late_command_cancellation_leaves_a_new_move_running(
    server_binary: Path,
) -> None:
    with running_station(server_binary) as station:

        async def exercise() -> None:
            async with (
                StationClient(station.server) as client,
                client.holding_lease("late cancellation") as lease,
            ):
                joints = list((await client.arm(ARM).state()).measured_joints)

                async def submit_move(operation_id: UUID, displacement: float) -> str:
                    response = await lease.send(
                        ARM,
                        MoveToJointsCommand(
                            joints=[joints[0] + displacement, *joints[1:]],
                            max_joint_speed=0.2,
                        ),
                        operation_id=operation_id,
                    )
                    assert isinstance(response, CommandAccepted)
                    return response.records[0].record.command_id

                async def wait_for_completion(command_id: str) -> str:
                    state = await client.wait_for_state(
                        lambda state: (
                            (record := state.arms[0].last_command) is not None
                            and record.command_id == command_id
                            and record.phase.kind != "pending"
                        )
                    )
                    record = state.arms[0].last_command
                    assert record is not None
                    outcome = command_record_outcome(record)
                    assert outcome is not None
                    return outcome.value

                original = uuid4()
                assert (
                    await wait_for_completion(await submit_move(original, 0.08))
                    == "completed"
                )
                newer = await submit_move(uuid4(), 0.35)
                await client.wait_for_state(
                    lambda state: (
                        (record := state.arms[0].active_command) is not None
                        and record.command_id == newer
                    )
                )
                await lease.cancel_operation(original)
                assert await wait_for_completion(newer) == "completed"

        asyncio.run(asyncio.wait_for(exercise(), timeout=15))


@pytest.mark.server
def test_one_operation_can_fence_admission_and_order_stream_updates(
    server_binary: Path,
) -> None:
    async def exercise(station_url: str) -> None:
        async with (
            StationClient(station_url) as client,
            client.holding_lease("stream operation") as lease,
        ):
            from hebbian_rua.client.contract.station_api import (
                StationStreamJointsCommand,
            )

            joints = list((await client.arm(ARM).state()).measured_joints)
            target = [joints[0] + 0.2, *joints[1:]]
            command = StationStreamJointsCommand(joints=target, max_joint_speed=0.1)
            fenced = uuid4()
            await lease.cancel_operation(fenced)
            response = await lease.send(ARM, command, operation_id=fenced)
            assert isinstance(response, CommandRefused)
            assert (
                await client.get_operation(fenced)
            ).root.state == "cancelled_before_admission"

            operation = uuid4()
            admitted = await lease.send(ARM, command, operation_id=operation)
            assert isinstance(admitted, CommandAccepted)
            assert await lease.send(ARM, command, operation_id=operation) == admitted
            updated = await lease.update_stream_goal(operation, 8, target)
            assert isinstance(updated, CommandAccepted)
            assert (
                updated.records[0].record.command_id
                == admitted.records[0].record.command_id
            )
            delayed = await lease.update_stream_goal(operation, 7, joints)
            assert isinstance(delayed, CommandRefused)
            assert delayed.reason is RejectionReason.invalid_target
            await lease.cancel_operation(operation)
            await client.wait_for_state(
                lambda state: (
                    state.arms[0].last_command is not None
                    and state.arms[0].last_command.command_id
                    == admitted.records[0].record.command_id
                    and state.arms[0].last_command.phase.kind != "pending"
                )
            )
            final = (await client.get_operation(operation)).root
            assert final.state == "finished"
            assert command_record_outcome(final.records[0].record) == "stopped"

    with running_station(server_binary) as station:
        asyncio.run(asyncio.wait_for(exercise(station.server), timeout=10))


@pytest.mark.server
def test_cancelling_a_move_stops_the_simulated_arm(server_binary: Path) -> None:
    with running_station(server_binary) as station:

        async def exercise_cancellation() -> None:
            async with (
                StationClient(station.server) as client,
                client.holding_lease("cancellation test") as lease,
            ):
                arm = client.arm(ARM)
                joints = list((await arm.state()).measured_joints)
                joints[0] += 0.4
                motion = asyncio.create_task(
                    lease.arm(ARM).move_to_joints(joints, max_joint_speed=0.1)
                )

                await asyncio.wait_for(
                    client.wait_for_state(
                        lambda state: state.arms[0].active_command is not None
                    ),
                    timeout=10,
                )
                motion.cancel()
                with pytest.raises(asyncio.CancelledError):
                    await motion
                await asyncio.wait_for(
                    client.wait_for_state(
                        lambda state: state.arms[0].active_command is None
                    ),
                    timeout=5,
                )
                assert (await arm.state()).last_command is not None

                joints = list((await arm.state()).measured_joints)
                joints[0] += 0.4
                tools = RobotTools(
                    client,
                    lease,
                    ARM,
                    configuration=RobotToolConfiguration(
                        tools=(
                            "action_sequence",
                            "preview_pose",
                            "move_arms",
                            "set_arm_gripper",
                            "rehome",
                            "stop",
                            "observe",
                        )
                    ),
                )
                batch = asyncio.create_task(
                    tools.execute(
                        [
                            RobotToolCall(
                                "move",
                                "move_arms",
                                {
                                    "goals": {
                                        ARM: {
                                            "joints_degrees": [
                                                math.degrees(joint) for joint in joints
                                            ]
                                        }
                                    },
                                    "max_joint_speed_degrees_per_second": math.degrees(
                                        0.1
                                    ),
                                },
                            ),
                            RobotToolCall("observe", "observe", {"wait_seconds": 0.1}),
                        ]
                    )
                )
                await asyncio.wait_for(
                    client.wait_for_state(
                        lambda state: state.arms[0].active_command is not None
                    ),
                    timeout=10,
                )
                batch.cancel()
                with pytest.raises(asyncio.CancelledError):
                    await batch
                await asyncio.wait_for(
                    client.wait_for_state(
                        lambda state: state.arms[0].active_command is None
                    ),
                    timeout=5,
                )

        asyncio.run(asyncio.wait_for(exercise_cancellation(), timeout=30))


@pytest.mark.server
def test_gripper_uses_shared_arm_speed_for_direct_and_combined_commands(
    server_binary: Path,
) -> None:
    with running_station(server_binary) as station:

        async def exercise() -> None:
            async with (
                StationClient(station.server) as client,
                client.holding_lease("gripper speed test") as lease,
            ):
                arm = lease.arm(ARM)
                direct = await arm.set_gripper(0.005, max_joint_speed=0.04)
                assert direct.completed
                assert not isinstance(direct.record, CommandRefused)
                assert direct.record.command.kind == "gripper"
                assert direct.record.command.joint_speed == pytest.approx(0.04)
                joints = (await client.arm(ARM).state()).measured_joints
                combined = await arm.run_action_sequence(
                    [
                        ActionSequenceMove(
                            goal=ActionSequenceJointsMovement(
                                joints=joints, max_joint_speed=0.03
                            ),
                            gripper_opening=0.0,
                            max_joint_speed=0.02,
                        )
                    ]
                )
                assert combined.phase.state == "completed"
                assert len(combined.steps[0]) == 2
                assert {record.record.command.kind for record in combined.steps[0]} == {
                    "move",
                    "gripper",
                }
                for entry in combined.steps[0]:
                    command = entry.record.command
                    assert isinstance(command, (RecordedMove, RecordedGripper))
                    assert command.joint_speed == pytest.approx(0.02)
                tools = RobotTools(
                    client,
                    lease,
                    ARM,
                    configuration=RobotToolConfiguration(
                        tools=("preview_pose", "move_arms", "set_arm_gripper")
                    ),
                )
                pose = (await client.arm(ARM).state()).tool
                assert pose is not None
                http_previews = await client.arm(ARM).preview_poses(
                    [
                        PosePreviewCandidate(
                            id="aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
                            pose=PreviewPose(position=pose.position),
                        )
                    ]
                )
                assert (
                    http_previews.results[0].id
                    == "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"
                )
                preview = await tools.execute(
                    [
                        RobotToolCall(
                            "preview",
                            "preview_pose",
                            {
                                "poses": [
                                    {
                                        "pose": {
                                            "position_mm": [
                                                coordinate * 1000
                                                for coordinate in pose.position
                                            ]
                                        },
                                    },
                                    {
                                        "pose": {"position_mm": [50000, 50000, 50000]},
                                    },
                                ]
                            },
                        )
                    ]
                )
                assert not preview[0].is_error
                assert isinstance(preview[0].data, dict)
                assert len(preview[0].data["results"]) == 2
                assert all(
                    set(item) == {"answer"} for item in preview[0].data["results"]
                )
                assert (
                    preview[0].data["results"][1]["answer"]["projection"]["outcome"][
                        "kind"
                    ]
                    == "projected"
                )
                assert (
                    preview[0].data["results"][1]["answer"]["projection"]["outcome"][
                        "within_tolerance"
                    ]
                    is False
                )
                gripper_result = await tools.execute(
                    [
                        RobotToolCall(
                            "partial",
                            "set_arm_gripper",
                            {
                                "arm": ARM,
                                "opening": 0.5,
                                "max_joint_speed_degrees_per_second": math.degrees(999),
                            },
                        )
                    ]
                )
                assert not gripper_result[0].is_error
                assert isinstance(gripper_result[0].data, dict)
                profile_speed = (await client.state()).max_joint_speed
                assert gripper_result[0].data["command"][
                    "joint_speed"
                ] == pytest.approx(profile_speed)
                assert isinstance(gripper_result[0].model_data, dict)
                assert gripper_result[0].model_data[
                    "joint_speed_degrees_per_second"
                ] == pytest.approx(round(math.degrees(profile_speed), 1))
                refused = await tools.execute(
                    [
                        RobotToolCall(
                            "unknown-arm",
                            "set_arm_gripper",
                            {"arm": "unknown", "opening": 0.5},
                        )
                    ]
                )
                assert refused[0].is_error
                assert "unknown" in refused[0].text.lower()
                examples = json.loads(
                    (
                        Path(__file__).parent / "fixtures/model-tool-simulation.json"
                    ).read_text()
                )
                tool_pose = (await client.arm(ARM).state()).tool
                assert tool_pose is not None
                limited = await tools.execute(
                    [
                        RobotToolCall(
                            "limited",
                            "move_arms",
                            {
                                "goals": {
                                    ARM: {
                                        "position_mm": examples[
                                            "limited_position_millimeters"
                                        ],
                                        "orientation": {
                                            "quaternion": tool_pose.quaternion
                                        },
                                    }
                                }
                            },
                        )
                    ]
                )
                assert limited[0].is_error
                assert isinstance(limited[0].data, dict)
                assert limited[0].data["status"] == "limited"

        asyncio.run(asyncio.wait_for(exercise(), timeout=30))

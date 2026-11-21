import asyncio
import shutil
from collections.abc import Generator

import httpx2
import pytest

from hebbian_rua.client.contract.station_api import (
    ActionSequenceArmAction,
    ActionSequenceJointsMovement,
    ActionSequenceRequest,
    ActionSequenceStep,
    ArmMode,
    CommandAccepted,
    DisableCommand,
)
from hebbian_rua.client.station_client import StationClient
from tests.fixture_station import (
    ARM,
    RunningStation,
    station_process,
    wait_until_station_live,
)
from tests.fixture_wait import wait_until_async

pytestmark = pytest.mark.server


async def recording_has_stopped(client: StationClient) -> bool:
    return (await client.recording_status()).root.state != "stopping"


@pytest.fixture(scope="module")
def station(tmp_path_factory: pytest.TempPathFactory) -> Generator[RunningStation]:
    if shutil.which("ffmpeg") is None:
        pytest.skip("ffmpeg is required to enable recording")
    recorder = tmp_path_factory.mktemp("recorder") / "recorder"
    recorder.write_text("#!/bin/sh\nexec sleep 30\n")
    recorder.chmod(0o755)
    with pytest.MonkeyPatch.context() as environment:
        environment.setenv("LIVEKIT_URL", "ws://127.0.0.1:1")
        environment.setenv("LIVEKIT_API_KEY", "test-key")
        environment.setenv("LIVEKIT_API_SECRET", "test-secret")
        with station_process(
            "--enable-recording", "--media-executable", str(recorder)
        ) as process:
            server = process.http_server()
            wait_until_station_live(server)
            yield RunningStation(server)


def test_recording_context_starts_and_stops_recording(station: RunningStation) -> None:
    async def record() -> None:
        async with StationClient(station.server) as client, client.recording() as name:
            assert station.get("/viewer/api/recordings").json()["active"] == name
            status = (await client.recording_status()).root
            assert status.state == "recording"
            assert status.name == name
            assert await client.ensure_recording() == name

    asyncio.run(record())


def test_recording_continues_during_a_action_sequence(station: RunningStation) -> None:
    async def exercise() -> None:
        async with StationClient(station.server) as client:
            state = await client.wait_for_state(
                lambda state: all(
                    arm.sampled_at_unix_seconds is not None for arm in state.arms
                )
            )
            if state.arms[0].mode is ArmMode.disabled:
                assert (await client.arm(ARM).enable()).completed
            async with (
                client.holding_lease("action_sequence recording") as lease,
                client.recording() as name,
            ):
                joints = (await client.arm(ARM).state()).measured_joints
                action_sequence_id = await lease.start_action_sequence(
                    ActionSequenceRequest(
                        steps=[
                            ActionSequenceStep(
                                arms={
                                    ARM: ActionSequenceArmAction(
                                        movement=ActionSequenceJointsMovement(
                                            joints=joints
                                        )
                                    )
                                }
                            )
                        ],
                    )
                )
                action_sequence = await client.wait_for_action_sequence(
                    action_sequence_id
                )
                assert action_sequence.phase.state == "completed", action_sequence
                status = (await client.recording_status()).root
                assert status.state == "recording"
                assert status.name == name

    asyncio.run(asyncio.wait_for(exercise(), timeout=30))


def test_previous_holder_cannot_stop_recording_after_the_lease_is_taken(
    station: RunningStation,
) -> None:
    async def handover() -> None:
        async with (
            StationClient(station.server) as first,
            StationClient(station.server) as second,
        ):
            state = await first.wait_for_state(
                lambda state: all(
                    arm.sampled_at_unix_seconds is not None for arm in state.arms
                )
            )
            if state.arms[0].mode is ArmMode.disabled:
                assert (await first.arm(ARM).enable()).completed
            await wait_until_async(lambda: recording_has_stopped(first))
            recording = await first.start_recording()
            async with (
                first.holding_lease("first") as first_lease,
                second.holding_lease("second", take=True) as second_lease,
            ):
                with pytest.raises(httpx2.HTTPStatusError) as refusal:
                    await first_lease.stop_recording()
                assert refusal.value.response.status_code == 409
                status = (await second.recording_status()).root
                assert status.state == "recording"
                assert status.name == recording
                await second_lease.stop_recording()

    asyncio.run(asyncio.wait_for(handover(), timeout=30))


def test_disabling_the_arm_leaves_the_lease_and_recording_running(
    station: RunningStation,
) -> None:
    async def disable() -> None:
        async with StationClient(station.server) as client:
            state = await client.wait_for_state(
                lambda state: all(
                    arm.sampled_at_unix_seconds is not None for arm in state.arms
                )
            )
            if state.arms[0].mode is ArmMode.disabled:
                assert (await client.arm(ARM).enable()).completed
            await wait_until_async(lambda: recording_has_stopped(client))
            recording_name = await client.start_recording()
            async with client.holding_lease("holder") as lease:
                assert isinstance(
                    await lease.send(ARM, DisableCommand()), CommandAccepted
                )

                async def arm_is_disabled() -> bool:
                    return (await client.arm(ARM).state()).mode is ArmMode.disabled

                await wait_until_async(arm_is_disabled, timeout=10)
                assert not client.lease_lost.is_set()
                assert (await client.state()).lease is not None
                status = (await client.recording_status()).root
                assert status.state == "recording"
                assert status.name == recording_name
            await client.stop_recording()

    asyncio.run(asyncio.wait_for(disable(), timeout=30))

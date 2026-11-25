import asyncio
import os
import shutil
import subprocess
from collections.abc import Generator
from pathlib import Path

import pytest

from hebbian_rua import HttpError
from hebbian_rua.client.contract.station_api import (
    ActionSequenceJointsMovement,
    ActionSequenceMove,
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
LIVEKIT_CREDENTIALS = {
    "LIVEKIT_URL": "ws://127.0.0.1:1",
    "LIVEKIT_API_KEY": "test-key",
    "LIVEKIT_API_SECRET": "test-secret",
}


@pytest.mark.parametrize("disable_recording", [False, True])
def test_station_requires_livekit_credentials(
    server_binary: Path, tmp_path: Path, disable_recording: bool
) -> None:
    result = subprocess.run(
        [server_binary, "serve", *(["--no-recording"] if disable_recording else [])],
        cwd=tmp_path,
        env=os.environ | dict.fromkeys(LIVEKIT_CREDENTIALS, ""),
        capture_output=True,
        text=True,
        timeout=5,
    )
    assert result.returncode != 0
    assert "LiveKit is required for station startup" in result.stderr
    assert "--local-only-no-remote-cameras" in result.stderr


@pytest.mark.parametrize("ffmpeg_on_path", [False, True], ids=["missing", "fails"])
def test_default_station_requires_working_ffmpeg(
    server_binary: Path, tmp_path: Path, ffmpeg_on_path: bool
) -> None:
    if ffmpeg_on_path:
        ffmpeg = tmp_path / "ffmpeg"
        ffmpeg.write_text("#!/bin/sh\nexit 1\n")
        ffmpeg.chmod(0o755)
    result = subprocess.run(
        [server_binary, "serve"],
        cwd=tmp_path,
        env=os.environ | LIVEKIT_CREDENTIALS | {"PATH": str(tmp_path)},
        capture_output=True,
        text=True,
        timeout=5,
    )
    assert result.returncode != 0
    assert "Recording requires FFmpeg on PATH" in result.stderr
    assert "--no-recording" in result.stderr


@pytest.mark.parametrize("local_cameras_only", [False, True], ids=["livekit", "local"])
def test_station_can_start_without_ffmpeg(
    server_binary: Path,
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    local_cameras_only: bool,
) -> None:
    monkeypatch.setenv("PATH", str(tmp_path))
    for name, value in LIVEKIT_CREDENTIALS.items():
        monkeypatch.setenv(name, "" if local_cameras_only else value)
    arguments = () if local_cameras_only else ("--no-recording",)
    with station_process(
        server_binary, *arguments, local_cameras_only=local_cameras_only
    ) as process:
        station = RunningStation(process.http_server())
        assert station.get("/recording/enabled").json() is False
        if not local_cameras_only:
            access = station.get("/media/access-token").json()
            assert access["server_url"] == LIVEKIT_CREDENTIALS["LIVEKIT_URL"]


async def recording_has_stopped(client: StationClient) -> bool:
    return (await client.recording_status()).root.state != "stopping"


@pytest.fixture(scope="module")
def station(
    tmp_path_factory: pytest.TempPathFactory, server_binary: Path
) -> Generator[RunningStation]:
    if shutil.which("ffmpeg") is None:
        pytest.skip("ffmpeg is required to enable recording")
    recorder = tmp_path_factory.mktemp("recorder") / "recorder"
    recorder.write_text("#!/bin/sh\nexec sleep 30\n")
    recorder.chmod(0o755)
    with pytest.MonkeyPatch.context() as environment:
        for name, value in LIVEKIT_CREDENTIALS.items():
            environment.setenv(name, value)
        with station_process(
            server_binary, "--media-executable", str(recorder), local_cameras_only=False
        ) as process:
            server = process.http_server()
            wait_until_station_live(server)
            station = RunningStation(server)
            assert station.get("/recording/enabled").json() is True
            yield station


def test_recording_context_starts_and_stops_recording(station: RunningStation) -> None:
    async def record() -> None:
        async with StationClient(station.server) as client:
            async with client.recording() as name:
                assert station.get("/recordings").json()["active"] == name
                assert await client.ensure_recording() == name

            async def recording_is_inactive() -> bool:
                status = (await client.recording_status()).root
                return status.state not in {"recording", "stopping"}

            await wait_until_async(recording_is_inactive)
            assert station.get("/recordings").json()["active"] is None

    asyncio.run(record())


def test_recording_continues_during_a_action_sequence(station: RunningStation) -> None:
    async def exercise() -> None:
        async with StationClient(station.server) as client:
            state = await client.wait_for_state(
                lambda state: all(
                    arm.sampled_unix_nano is not None for arm in state.arms
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
                                    ARM: ActionSequenceMove(
                                        goal=ActionSequenceJointsMovement(joints=joints)
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
                    arm.sampled_unix_nano is not None for arm in state.arms
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
                with pytest.raises(HttpError) as refusal:
                    await first_lease.stop_recording()
                assert refusal.value.status == 409
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
                    arm.sampled_unix_nano is not None for arm in state.arms
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

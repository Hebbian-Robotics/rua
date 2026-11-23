import asyncio
import contextlib
import os
import shutil
import signal
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from uuid import uuid4

import httpx2
import psutil
import pytest

from hebbian_rua.client.contract.station_api import (
    ArmMode,
    CommandAccepted,
    CommandRefused,
    EnableCommand,
    MoveToJointsCommand,
)
from hebbian_rua.client.station_client import StationClient
from tests.fixture_station import (
    ARM,
    MUJOCO_PROFILE,
    station_process,
    wait_until_station_live,
)
from tests.fixture_wait import wait_for_value, wait_until, wait_until_async


def controller_pid(station_pid: int) -> int | None:
    with contextlib.suppress(psutil.NoSuchProcess):
        for child in psutil.Process(station_pid).children():
            with contextlib.suppress(psutil.NoSuchProcess):
                arguments = child.cmdline()
                if len(arguments) > 1 and arguments[1] == "controller":
                    return child.pid
    return None


@pytest.mark.server
def test_station_termination_waits_for_clean_controller_exit(
    server_binary: Path,
) -> None:
    with station_process(server_binary) as station:
        server = station.http_server()
        wait_until_station_live(server)

        async def enable() -> None:
            async with StationClient(server) as client:
                assert (await client.arm(ARM).enable()).completed

        asyncio.run(enable())
        station.process.terminate()
        assert station.process.wait(timeout=25) == 0
        wait_until(
            lambda: any(
                "Controller shut down cleanly" in line
                for line in list(station.recent_errors)
            ),
            timeout=2,
            message="station did not report a clean controller exit",
        )


@pytest.mark.server
def test_draining_station_rejects_new_work_and_keeps_program_outcomes(
    server_binary: Path,
) -> None:
    with station_process(server_binary) as station:
        server = station.http_server()
        wait_until_station_live(server)
        child = wait_for_value(lambda: controller_pid(station.process.pid))
        with httpx2.Client(base_url=server, timeout=3) as client:
            request = {
                "station_instance_id": client.get("/instance").json()["id"],
                "request_id": str(uuid4()),
                "compilation_id": str(uuid4()),
                "lease_token": "unused",
            }
            original = client.post("/compilations/run", json=request)
            assert original.status_code == 404, original.text

            os.kill(child, signal.SIGSTOP)
            try:
                station.process.terminate()
                wait_until(
                    lambda: (
                        client.post("/compilations/compile", json={}).status_code == 503
                    ),
                    timeout=3,
                    message="station kept accepting compilation requests during shutdown",
                )
                new_request = {**request, "request_id": str(uuid4())}
                new_submission = client.post("/compilations/run", json=new_request)
                assert new_submission.status_code == 503, new_submission.text
                replayed = client.post("/compilations/run", json=request)
                assert (replayed.status_code, replayed.text) == (404, original.text)
                assert client.get("/health").status_code == 200
            finally:
                os.kill(child, signal.SIGCONT)
        assert station.process.wait(timeout=10) == 0


@pytest.mark.server
def test_second_station_cannot_overwrite_an_active_work_directory(
    tmp_path: Path,
    server_binary: Path,
) -> None:
    # WHY: Unix socket paths have a short length limit.
    with tempfile.TemporaryDirectory(prefix="rua-work-", dir="/tmp") as directory:
        work_directory = Path(directory)
        with station_process(
            server_binary, work_directory=work_directory
        ) as first_station:
            server = first_station.http_server()
            wait_until_station_live(server)
            config_path = work_directory / f"{ARM}.json"
            original_config = config_path.read_bytes()
            second = subprocess.run(
                [
                    str(server_binary),
                    "serve",
                    "--profile",
                    str(MUJOCO_PROFILE),
                    "--no-physics-stand-in",
                    "--http-address",
                    server.removeprefix("http://"),
                    "--media-address",
                    "127.0.0.1:0",
                    "--work-directory",
                    str(work_directory),
                    "--trace-directory",
                    str(tmp_path / "second-traces"),
                ],
                capture_output=True,
                text=True,
                timeout=10,
                check=False,
            )
            assert config_path.read_bytes() == original_config
            assert second.returncode != 0
            assert "Cannot lock work directory" in second.stderr

        with station_process(server_binary, work_directory=work_directory) as restarted:
            wait_until_station_live(restarted.http_server())


@pytest.mark.server
@pytest.mark.skipif(sys.platform != "linux", reason="requires /proc/self/exe")
def test_station_reconnects_disabled_using_its_running_binary(
    tmp_path: Path, server_binary: Path
) -> None:
    installed_binary = tmp_path / "station-before-upgrade"
    shutil.copy2(server_binary, installed_binary)
    with station_process(installed_binary) as station:
        server = station.http_server()
        wait_until_station_live(server)
        installed_binary.unlink()
        process_id = wait_for_value(lambda: controller_pid(station.process.pid))
        command_line = psutil.Process(process_id).cmdline()
        arm_argument = next(
            argument for argument in command_line if argument.startswith("arm=")
        )
        configuration_path = Path(arm_argument.split("=", 1)[1])
        configuration = configuration_path.read_bytes()

        async def exercise() -> None:
            async with (
                StationClient(server) as client,
                client.holding_lease("recovery test") as lease,
            ):
                assert (await client.arm(ARM).enable()).completed
                target = list((await client.arm(ARM).state()).measured_joints)
                target[0] += 0.3
                from uuid import uuid4

                operation = uuid4()
                motion = await lease.send(
                    ARM,
                    MoveToJointsCommand(joints=target, max_joint_speed=0.05),
                    operation_id=operation,
                )
                assert isinstance(motion, CommandAccepted)
                os.kill(process_id, signal.SIGKILL)
                await asyncio.wait_for(client.lease_lost.wait(), timeout=10)
                assert station.process.poll() is None
                assert httpx2.get(f"{server}/health").status_code == 200
                assert isinstance(
                    await lease.send(ARM, EnableCommand()), CommandRefused
                )

                async def command_interrupted() -> bool:
                    response = httpx2.get(f"{server}/operations/{operation}")
                    if response.status_code != 200:
                        return False
                    phase = response.json()["records"][0]["record"]["phase"]
                    return phase["kind"] == "outcome_unknown"

                await wait_until_async(command_interrupted, timeout=10)

                async def unavailable() -> bool:
                    return (
                        httpx2.get(f"{server}/controller").json()["phase"]
                        == "unavailable"
                    )

                await wait_until_async(unavailable, timeout=10)
                assert (
                    httpx2.post(
                        f"{server}/controller/reconnect",
                        json={"supported": False},
                    ).status_code
                    == 400
                )
                configuration_path.unlink()
                try:
                    assert (
                        httpx2.post(
                            f"{server}/controller/reconnect",
                            json={"supported": True},
                        ).status_code
                        == 200
                    )

                    async def startup_failed() -> bool:
                        status = httpx2.get(f"{server}/controller").json()
                        return (
                            status["phase"] == "unavailable"
                            and "startup" in status["reason"]
                        )

                    await wait_until_async(startup_failed, timeout=15)
                    assert station.process.poll() is None
                finally:
                    configuration_path.write_bytes(configuration)
                assert (
                    httpx2.post(
                        f"{server}/controller/reconnect",
                        json={"supported": True},
                    ).status_code
                    == 200
                )

                async def online() -> bool:
                    return (
                        httpx2.get(f"{server}/controller").json()["phase"] == "online"
                    )

                await wait_until_async(online, timeout=15)

                async def disabled() -> bool:
                    return (await client.arm(ARM).state()).mode == ArmMode.disabled

                await wait_until_async(disabled, timeout=10)
                await asyncio.sleep(0.2)
                assert (await client.arm(ARM).state()).mode == ArmMode.disabled
                assert isinstance(
                    await lease.send(ARM, EnableCommand()), CommandRefused
                )
                assert isinstance(
                    await lease.send(ARM, MoveToJointsCommand(joints=target)),
                    CommandRefused,
                )

            async with StationClient(server) as operator:
                assert (await operator.arm(ARM).enable()).completed

        asyncio.run(asyncio.wait_for(exercise(), timeout=45))


@pytest.mark.server
def test_short_delay_recovers_but_a_stalled_controller_is_stopped(
    server_binary: Path,
) -> None:
    with station_process(server_binary) as station:
        server = station.http_server()
        wait_until_station_live(server)
        process_id = wait_for_value(lambda: controller_pid(station.process.pid))

        with httpx2.Client(base_url=server) as client:
            try:
                os.kill(process_id, signal.SIGSTOP)
                time.sleep(0.1)
                resumed_at = time.time_ns()
                os.kill(process_id, signal.SIGCONT)

                def fresh_state() -> bool:
                    arm = client.get("/state").json()["arms"][0]
                    sampled_at = arm["sampled_unix_nano"]
                    return sampled_at is not None and int(sampled_at) > resumed_at

                wait_until(fresh_state, timeout=5)
                assert controller_pid(station.process.pid) == process_id
                assert client.get("/controller").json()["phase"] == "online"

                os.kill(process_id, signal.SIGSTOP)

                def controller_stopped() -> bool:
                    status = client.get("/controller").json()
                    return (
                        status["phase"] == "unavailable"
                        and status["guard_released"]
                        and controller_pid(station.process.pid) is None
                    )

                wait_until(controller_stopped, timeout=10)
                assert station.process.poll() is None
                assert client.get("/health").status_code == 200
            finally:
                with contextlib.suppress(ProcessLookupError):
                    os.kill(process_id, signal.SIGCONT)

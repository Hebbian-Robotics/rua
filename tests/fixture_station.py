import asyncio
import collections
import subprocess
import sys
import tempfile
import threading
from collections.abc import Generator
from contextlib import contextmanager
from dataclasses import dataclass
from pathlib import Path

import httpx2

from examples.mujoco_sim.arm_config import ArmConfig
from hebbian_rua.client.contract.station_api import (
    StationState,
)
from hebbian_rua.client.station_client import StationClient
from tests.fixture_wait import wait_until

REPOSITORY_ROOT = Path(__file__).resolve().parents[1]
ARM = "arm"
MUJOCO_PROFILE = REPOSITORY_ROOT / "examples/mujoco_sim/profiles/single.toml"
HTTP_ANNOUNCEMENT = "HTTP on "
STOP_SECONDS = 10.0


def resolve_simulator_arm(
    executable: Path, profile: Path = MUJOCO_PROFILE
) -> ArmConfig:
    config = subprocess.run(
        [
            str(executable),
            "config",
            "--profile",
            str(profile),
            "--arm",
            ARM,
            "--urdf-limits",
        ],
        check=True,
        capture_output=True,
    )
    return ArmConfig.model_validate_json(config.stdout)


class StationProcess:
    def __init__(self, arguments: list[str], executable: Path) -> None:
        self.process = subprocess.Popen(
            [str(executable), "serve", *arguments],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.PIPE,
            text=True,
        )
        self.recent_errors: collections.deque[str] = collections.deque(maxlen=50)
        threading.Thread(target=self.relay_errors, daemon=True).start()

    def relay_errors(self) -> None:
        assert self.process.stderr is not None
        for line in self.process.stderr:
            self.recent_errors.append(line.rstrip())
            sys.stderr.write(line)

    def http_server(self) -> str:
        announced: list[str] = []

        def announcement_seen() -> bool:
            announced[:] = [
                line.removeprefix(HTTP_ANNOUNCEMENT)
                for line in list(self.recent_errors)
                if line.startswith(HTTP_ANNOUNCEMENT)
            ]
            assert self.process.poll() is None, "\n".join(self.recent_errors)
            return bool(announced)

        wait_until(
            announcement_seen,
            timeout=30,
            interval=0.02,
            message="the station never announced its HTTP address",
        )
        return announced[0]

    def stop(self) -> None:
        if self.process.poll() is None:
            self.process.terminate()
            try:
                self.process.wait(STOP_SECONDS)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait()


@dataclass(frozen=True)
class RunningStation:
    server: str

    def get(self, path: str) -> httpx2.Response:
        response = httpx2.get(f"{self.server}{path}", timeout=10)
        response.raise_for_status()
        return response


def wait_until_station_live(server: str, seconds: float = 30.0) -> None:

    def every_arm_reported() -> bool:
        response = httpx2.get(f"{server}/state", timeout=10)
        if response.status_code != 200:
            return False
        arms = StationState.model_validate_json(response.content).arms
        return bool(arms) and all(
            arm.sampled_at_unix_seconds is not None for arm in arms
        )

    wait_until(
        every_arm_reported,
        timeout=seconds,
        interval=0.05,
        message="the station never reported the arm's joints",
    )


@contextmanager
def station_process(
    executable: Path,
    *extra_arguments: str,
    profile: Path = MUJOCO_PROFILE,
    work_directory: Path | None = None,
) -> Generator[StationProcess]:
    with tempfile.TemporaryDirectory(prefix="hebbian-test-") as scratch:
        station = StationProcess(
            [
                "--profile",
                str(profile),
                "--no-physics-stand-in",
                "--http-address",
                "127.0.0.1:0",
                *(
                    ("--media-address", "127.0.0.1:0")
                    if "--media-address" not in extra_arguments
                    else ()
                ),
                "--work-directory",
                str(
                    work_directory
                    if work_directory is not None
                    else Path(scratch) / "work"
                ),
                "--recordings-directory",
                str(Path(scratch) / "recordings"),
                *extra_arguments,
            ],
            executable,
        )
        try:
            yield station
        finally:
            station.stop()


@contextmanager
def running_station(
    executable: Path, *, enabled: bool = True, media_address: str | None = None
) -> Generator[RunningStation]:
    arguments = ("--media-address", media_address) if media_address is not None else ()
    with station_process(executable, *arguments) as station:
        server = station.http_server()
        wait_until_station_live(server)
        if enabled:

            async def enable() -> None:
                async with StationClient(server) as client:
                    enabled_result = await client.arm(ARM).enable()
                    assert enabled_result.completed, enabled_result

            asyncio.run(enable())
        yield RunningStation(server)

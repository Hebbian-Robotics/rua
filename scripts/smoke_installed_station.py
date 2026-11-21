import argparse
import asyncio
import json
import os
import shutil
import socket
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from urllib.error import URLError
from urllib.request import urlopen

from hebbian_rua.client.contract.station_api import (
    ArmMode,
    CommandRecord,
    PoseProjection,
    QuaternionOrientation,
)
from hebbian_rua.client.station_client import StationClient


def available_loopback_port() -> int:
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        return listener.getsockname()[1]


def check_installed_licenses(*, without_viewer: bool, require_media: bool) -> None:
    for component in ("rust",) if without_viewer else ("viewer", "rust"):
        license_directory = (
            Path(sys.prefix) / "share/hebbian-robot-use/licenses" / component
        )
        packages = json.loads((license_directory / "manifest.json").read_text())
        if not packages:
            raise RuntimeError(f"{component} dependency license manifest is empty")
        for package in packages:
            if not package["files"] or any(
                not (license_directory / package["directory"] / filename).is_file()
                for filename in package["files"]
            ):
                raise RuntimeError(
                    f"{component} license files are missing for {package['name']}"
                )
    if require_media:
        media_executable = shutil.which("hebbian-robot-use-media")
        if media_executable is None:
            raise RuntimeError("The media executable is not on PATH")
        webrtc_license = (
            Path(media_executable).resolve().parent.parent
            / "share/hebbian-robot-use-media/licenses/webrtc/LICENSE.md"
        )
        if not webrtc_license.is_file():
            raise RuntimeError("The installed media package has no WebRTC license")


async def check_station(address: str, *, exercise_motion: bool) -> None:
    async with StationClient(address) as station:
        arm = station.arm("arm")
        deadline = time.monotonic() + 10
        last_mode: ArmMode | None = None
        while time.monotonic() < deadline:
            last_mode = (await arm.state()).mode
            if last_mode == ArmMode.disabled:
                break
            await asyncio.sleep(0.1)
        else:
            raise RuntimeError(f"Simulated arm did not become ready: {last_mode}")
        if exercise_motion:
            async with station.holding_lease("release smoke test") as lease:
                enabled = await arm.enable()
                if not enabled.completed:
                    raise RuntimeError(f"Simulated enable failed: {enabled}")
                target = [0.22, 0.0, 0.10]
                preview = await arm.preview_pose(target)
                if (
                    not isinstance(preview, PoseProjection)
                    or preview.outcome.kind != "projected"
                    or not preview.outcome.within_tolerance
                ):
                    raise RuntimeError(f"Simulated preview failed: {preview}")
                orientation = QuaternionOrientation(
                    quaternion=preview.outcome.pose.quaternion
                )
                move = await lease.arm("arm").move_to_pose(target, orientation)
                if not move.completed:
                    raise RuntimeError(f"Simulated move failed: {move}")
                if isinstance(move.record, CommandRecord) and (
                    move.record.pose_limited or move.record.path_limited
                ):
                    raise RuntimeError(f"Simulated move was limited: {move}")
                gripper = await lease.arm("arm").set_gripper(1.0)
                if not gripper.completed:
                    raise RuntimeError(f"Simulated gripper failed: {gripper}")
        print(f"Installed station and client connected: {last_mode}")


def wait_for_station(
    process: subprocess.Popen[bytes], address: str, *, without_viewer: bool
) -> None:
    deadline = time.monotonic() + 20
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise RuntimeError("Station exited before the HTTP server started")
        try:
            readiness_path = "/state" if without_viewer else "/viewer/"
            with urlopen(f"{address}{readiness_path}", timeout=1) as response:
                if response.status == 200:
                    break
        except (OSError, URLError):
            time.sleep(0.1)
    else:
        raise RuntimeError("Station HTTP readiness check failed")


def main() -> None:
    parser = argparse.ArgumentParser(
        description=(
            "Check an installed client and station outside the source checkout.\n"
            "\n"
            "Run with the Python environment containing both locally built wheels. The\n"
            "station uses its lag simulator; no hardware or camera is opened.\n"
        )
    )
    parser.add_argument("--require-media", action="store_true")
    parser.add_argument("--without-viewer", action="store_true")
    parser.add_argument("--exercise-motion", action="store_true")
    parser.add_argument(
        "--example", type=Path, help="Run an example with the installed client"
    )
    arguments = parser.parse_args()
    executable = shutil.which("hebbian-robot-use-server")
    if executable is None:
        raise SystemExit("Install the station wheel and put its scripts on PATH")

    # WHY: The OS can reuse a port after the listener closes, so avoid choosing the
    # same port for the station and media service.
    port = available_loopback_port()
    media_port = available_loopback_port()
    while media_port == port:
        media_port = available_loopback_port()
    address = f"http://127.0.0.1:{port}"
    environment = {
        key: value
        for key, value in os.environ.items()
        if not key.startswith(("HEBBIAN_", "LIVEKIT_"))
    }

    def resolved_simulator_arm(profile: str, work_directory: Path) -> dict:
        result = subprocess.run(
            [
                executable,
                "config",
                "--profile",
                profile,
                "--arm",
                "arm",
                "--urdf-limits",
            ],
            cwd=work_directory,
            env=environment,
            capture_output=True,
            check=True,
        )
        return json.loads(result.stdout)

    check_installed_licenses(
        without_viewer=arguments.without_viewer, require_media=arguments.require_media
    )

    with tempfile.TemporaryDirectory(prefix="hebbian-wheel-smoke-") as directory:
        profile_path = Path(directory) / "arm.toml"
        profile_path.write_text('extends = "so101-sim"\nname = "wheel-smoke"\n')
        profile = resolved_simulator_arm("so101-sim", Path(directory))
        if profile["station"]["robot"] != "so101-sim":
            raise RuntimeError("Installed simulation profile has the wrong name")
        if profile["simulation_model"] is None or not Path(profile["urdf"]).is_file():
            raise RuntimeError("Installed simulator profile or robot model is missing")
        local_profile = resolved_simulator_arm(str(profile_path), Path(directory))
        if local_profile["urdf"] != profile["urdf"]:
            raise RuntimeError(
                "External profile did not inherit the bundled robot model"
            )
        log_path = Path(directory) / "station.log"
        with log_path.open("w+") as log:
            process = subprocess.Popen(
                [
                    executable,
                    "serve",
                    "--profile",
                    str(profile_path),
                    "--no-physics-stand-in",
                    "--http-address",
                    f"127.0.0.1:{port}",
                    "--media-address",
                    f"127.0.0.1:{media_port}",
                ],
                cwd=directory,
                env=environment,
                stdout=log,
                stderr=subprocess.STDOUT,
            )
            try:
                wait_for_station(
                    process, address, without_viewer=arguments.without_viewer
                )
                if arguments.require_media:
                    deadline = time.monotonic() + 10
                    while time.monotonic() < deadline:
                        try:
                            with urlopen(
                                f"http://127.0.0.1:{media_port}/cameras", timeout=1
                            ) as response:
                                if response.status == 200:
                                    break
                        except (OSError, URLError):
                            time.sleep(0.1)
                    else:
                        raise RuntimeError("Media service did not serve cameras")

                asyncio.run(
                    check_station(address, exercise_motion=arguments.exercise_motion)
                )
                if arguments.example:
                    subprocess.run(
                        [sys.executable, "-I", str(arguments.example.resolve())],
                        cwd=directory,
                        env=environment | {"HEBBIAN_STATION_URL": address},
                        check=True,
                        timeout=60,
                    )
            except Exception:
                log.flush()
                print(log_path.read_text())
                raise
            finally:
                process.terminate()
                process.wait(timeout=15)


if __name__ == "__main__":
    main()

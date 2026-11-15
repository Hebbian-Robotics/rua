"""Verify an installed release with an independently connected socket peer.

Run with the Python from the temporary wheel-only environment. This verifies
transport and packaging, not physics or physical hardware.
"""

import asyncio
import hashlib
import importlib.metadata
import importlib.util
import json
import os
import shutil
import signal
import socket
import subprocess
import sysconfig
import tempfile
import threading
import time
import tomllib
from pathlib import Path

from hebbian_rua import StationClient


def available_port() -> int:
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        return int(listener.getsockname()[1])


def simulator_peer(socket_path: Path, stopped: threading.Event) -> None:
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as connection:
        connection.settimeout(1)
        connection.connect(str(socket_path))
        state = {
            "positions": [0.0, -1.0, 1.0, 0.5, 0.0],
            "velocities": [0.0] * 5,
            "efforts": [0.0] * 5,
            "gripper_angle": 0.0,
        }
        hello = {
            "protocol_version": 1,
            "joint_names": [
                "shoulder_pan",
                "shoulder_lift",
                "elbow_flex",
                "wrist_flex",
                "wrist_roll",
                "gripper",
            ],
            "state": state,
        }
        connection.sendall(json.dumps(hello).encode() + b"\n")
        with connection.makefile("rb") as reader:
            while not stopped.is_set():
                try:
                    step = reader.readline()
                    if not step:
                        return
                    connection.sendall(json.dumps(state).encode() + b"\n")
                except (OSError, TimeoutError):
                    return


async def check_client(address: str) -> None:
    async with StationClient(address) as station:
        deadline = time.monotonic() + 20
        while time.monotonic() < deadline:
            state = await station.arm("follower").state()
            if state.measured_joints:
                print(
                    "Installed SDK received arm feedback from a user-owned socket connection"
                )
                return
            await asyncio.sleep(0.05)
        raise RuntimeError("No simulator feedback reached the Python client")


def main() -> None:
    own_distributions = sorted(
        distribution.metadata["Name"]
        for distribution in importlib.metadata.distributions()
        if distribution.metadata["Name"].startswith("hebbian-")
    )
    if own_distributions != ["hebbian-rua"]:
        raise RuntimeError(f"Expected one public distribution: {own_distributions}")
    for unwanted in ("hebbian_robot_use_client", "mujoco", "so101_setup", "rustypot"):
        if importlib.util.find_spec(unwanted) is not None:
            raise RuntimeError(f"Unexpected runtime dependency: {unwanted}")
    command = shutil.which("rua")
    media = shutil.which("hebbian-robot-use-media")
    if command is None or media is None:
        raise RuntimeError("The installed wheel is missing its commands")
    subprocess.run([command, "--help"], check=True, stdout=subprocess.DEVNULL)
    subprocess.run([media, "--help"], check=True, stdout=subprocess.DEVNULL)
    shared_assets = Path(sysconfig.get_path("data")) / "share/hebbian-robot-use"
    asset_pin = tomllib.loads((shared_assets / "robots/so101-assets.toml").read_text())
    for filename, digest in asset_pin["files"].items():
        asset = shared_assets / "robots/so101" / filename
        if hashlib.sha256(asset.read_bytes()).hexdigest() != digest:
            raise RuntimeError(
                f"Installed robot asset does not match its pin: {filename}"
            )
    print("Installed models, license and attribution match the pinned SO-101 release")
    port = available_port()
    stopped = threading.Event()
    with tempfile.TemporaryDirectory(prefix="rua-installed-check-") as temporary:
        directory = Path(temporary)
        environment = {
            name: value
            for name, value in os.environ.items()
            if not name.startswith(("HEBBIAN_", "LIVEKIT_"))
        }
        with (directory / "station.log").open("w+") as log:
            process = subprocess.Popen(
                [
                    command,
                    "serve",
                    "--profile",
                    "so101-socket",
                    "--work-directory",
                    temporary,
                    "--http-address",
                    f"127.0.0.1:{port}",
                    "--media-address",
                    "127.0.0.1:0",
                ],
                cwd=directory,
                env=environment,
                stdout=log,
                stderr=subprocess.STDOUT,
                start_new_session=True,
            )
            peer: threading.Thread | None = None
            try:
                socket_path = directory / "follower.plant.sock"
                deadline = time.monotonic() + 20
                while not socket_path.exists():
                    if process.poll() is not None or time.monotonic() > deadline:
                        raise RuntimeError(
                            "Installed station did not open the simulator socket"
                        )
                    time.sleep(0.05)
                peer = threading.Thread(
                    target=simulator_peer, args=(socket_path, stopped), daemon=True
                )
                peer.start()
                asyncio.run(check_client(f"http://127.0.0.1:{port}"))
            except Exception:
                log.flush()
                log.seek(0)
                print(log.read())
                raise
            finally:
                stopped.set()
                if process.poll() is None:
                    os.killpg(process.pid, signal.SIGTERM)
                    try:
                        process.wait(timeout=15)
                    except subprocess.TimeoutExpired:
                        os.killpg(process.pid, signal.SIGKILL)
                        process.wait()
                if peer is not None:
                    peer.join(timeout=2)
    print(
        "Single-wheel install, SDK, server, media and external simulator connection passed"
    )


if __name__ == "__main__":
    main()

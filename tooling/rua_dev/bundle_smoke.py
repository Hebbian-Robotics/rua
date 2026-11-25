# CONSTRAINT: This release smoke check verifies transport and packaging, not
# physical robot behavior or simulator physics.
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
from hebbian_rua.client.contract.station_api import (
    ActionSequenceJointsMovement,
    ActionSequenceMove,
    ActionSequenceRequest,
    ActionSequenceStep,
    ActionSequenceStopped,
    CompilationArtifact,
    PreparationFailure,
    ProgramCompiled,
    RejectionReason,
)


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


async def check_compiler(station: StationClient, joints: list[float]) -> None:
    source = ActionSequenceRequest(
        steps=[
            ActionSequenceStep(
                arms={
                    "arm": ActionSequenceMove(
                        goal=ActionSequenceJointsMovement(joints=joints)
                    )
                }
            )
        ],
    )
    outcome = await station.compile_action_sequence(source)
    if not isinstance(outcome, ProgramCompiled):
        raise RuntimeError(f"Installed compiler rejected valid source: {outcome}")
    exported = CompilationArtifact.model_validate_json(
        outcome.artifact.model_dump_json()
    )
    imported = await station.import_compilation(exported)
    if not isinstance(imported, ProgramCompiled):
        raise RuntimeError(f"Installed compiler rejected its export: {imported}")
    if imported.artifact.compilation_id == exported.compilation_id:
        raise RuntimeError("Imported source reused a foreign compilation identity")
    async with station.holding_lease("installed compiler check") as lease:
        execution_id = await lease.start_compiled_sequence(
            imported.artifact.compilation_id
        )
        execution = await station.wait_for_action_sequence(execution_id)
        if not (
            isinstance(execution.phase, ActionSequenceStopped)
            and isinstance(execution.phase.failure, PreparationFailure)
            and execution.phase.failure.reason == RejectionReason.not_enabled
        ):
            raise RuntimeError("Compiled execution bypassed disabled-arm preparation")
    print(
        "Installed SDK and native compiler agree on import, identity and live preparation"
    )


async def check_client(address: str) -> None:
    async with StationClient(address) as station:
        deadline = time.monotonic() + 20
        while time.monotonic() < deadline:
            state = await station.arm("arm").state()
            if state.measured_joints:
                print(
                    "Installed SDK received arm feedback from a user-owned socket connection"
                )
                await check_compiler(station, list(state.measured_joints))
                return
            await asyncio.sleep(0.05)
        raise RuntimeError("No simulator feedback reached the Python client")


def check_station_cli(station_cli: str, address: str) -> None:
    result = subprocess.run(
        [station_cli, "--server", address, "state"],
        capture_output=True,
        text=True,
        check=True,
    )
    arm_names = [arm["arm"] for arm in json.loads(result.stdout)["arms"]]
    if arm_names != ["arm"]:
        raise RuntimeError(f"Installed CLI did not report the arm: {result.stdout}")
    print("Installed CLI read station state")


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
    media = shutil.which("hebbian-rua-media")
    station_cli = shutil.which("hebbian-rua")
    if command is None or media is None or station_cli is None:
        raise RuntimeError("The installed wheel is missing its commands")
    for executable in (command, media, station_cli):
        subprocess.run([executable, "--help"], check=True, stdout=subprocess.DEVNULL)
    shared_assets = Path(sysconfig.get_path("data")) / "share/hebbian-rua"
    asset_pin = tomllib.loads((shared_assets / "robots/so101-assets.toml").read_text())
    model_directory = shared_assets / "robots/so101"
    installed_files = {
        path.relative_to(model_directory).as_posix()
        for path in model_directory.rglob("*")
        if path.is_file()
    }
    if installed_files != asset_pin["files"].keys():
        raise RuntimeError("Installed robot assets differ from the wheel allowlist")
    for filename, digest in asset_pin["files"].items():
        asset = model_directory / filename
        if hashlib.sha256(asset.read_bytes()).hexdigest() != digest:
            raise RuntimeError(
                f"Installed robot asset does not match its pin: {filename}"
            )
    print("Installed models, license and attribution match the pinned SO-101 release")
    port = available_port()
    stopped = threading.Event()
    with tempfile.TemporaryDirectory(
        prefix="rua-installed-check-", dir="/tmp"
    ) as temporary:
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
                    "--local-only-no-remote-cameras",
                    "--profile",
                    "so101-sim-only-no-real-arm",
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
                socket_path = directory / "arm.plant.sock"
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
                check_station_cli(station_cli, f"http://127.0.0.1:{port}")
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
        "Single-wheel install, SDK, CLI, server, media and external simulator connection passed"
    )


if __name__ == "__main__":
    main()

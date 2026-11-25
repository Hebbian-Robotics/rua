import json
import socket
import tempfile
from pathlib import Path

import httpx2
import pytest

from examples.mujoco_sim.arm_config import load_station_arms
from hebbian_rua.simulator import PlantConnection, PlantDisconnected, SimState
from tests.fixture_station import MUJOCO_PROFILE, StationProcess


def test_simulator_exchanges_plant_messages() -> None:
    with tempfile.TemporaryDirectory(prefix="rua-sim-", dir="/tmp") as directory:
        exchange_plant_messages(Path(directory))


def exchange_plant_messages(socket_directory: Path) -> None:
    socket_path = socket_directory / "arm.plant.sock"
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as listener:
        listener.bind(str(socket_path))
        listener.listen()
        with PlantConnection.connect(socket_directory, "arm") as connection:
            station, _ = listener.accept()
            with station, station.makefile("rwb") as lines:
                state = SimState(positions=[0.1], velocities=[0.0], efforts=[0.0])
                connection.send_hello(["joint"], state)
                assert b'"protocol_version":1' in lines.readline()
                lines.write(b'{"positions":[0.2],"seconds":0.01,"powered":true}\n')
                lines.flush()
                step = connection.read_step()
                assert step.positions == [0.2]
                assert step.powered
                connection.send_state(state)
                assert SimState.model_validate_json(lines.readline()) == state
                station.shutdown(socket.SHUT_RDWR)
                with pytest.raises(PlantDisconnected):
                    connection.read_step()


def test_simulator_rejects_invalid_arm_socket_name(tmp_path: Path) -> None:
    with pytest.raises(ValueError, match="socket name"):
        PlantConnection.connect(tmp_path, "../arm")


def test_simulator_and_station_state_use_the_selected_arm_in_a_reused_directory(
    server_binary: Path, tmp_path: Path
) -> None:
    profile = tmp_path / "workbench.toml"
    profile.write_text(
        f"extends = {json.dumps(str(MUJOCO_PROFILE))}\n"
        'arms = ["bench_a", "bench_b"]\n'
        "[base_poses.bench_b]\n"
        "position_meters = [0.025, 0.19, 0.04]\n"
        "yaw_radians = 0.4\n"
    )
    expected_base = {"position_meters": [0.025, 0.19, 0.04], "yaw_radians": 0.4}
    with tempfile.TemporaryDirectory(prefix="rua-sim-", dir="/tmp") as directory:
        work = Path(directory)
        (work / "bench_a.json").write_text("obsolete profile")
        (work / ".simulator-arms.json").write_text('["bench_a"]')
        station = StationProcess(
            [
                "--local-only-no-remote-cameras",
                "--profile",
                str(profile),
                "--arm",
                "bench_b",
                "--work-directory",
                directory,
                "--http-address",
                "127.0.0.1:0",
                "--media-address",
                "127.0.0.1:0",
                "--trace-directory",
                str(work / "traces"),
            ],
            executable=server_binary,
        )
        try:
            address = station.http_server()
            configs = load_station_arms(work)
            assert list(configs) == ["bench_b"]
            base_pose = configs["bench_b"].base_pose
            assert base_pose is not None
            assert base_pose.model_dump(mode="json") == expected_base
            response = httpx2.get(f"{address}/state", timeout=5)
            response.raise_for_status()
            (arm,) = response.json()["arms"]
            assert arm["arm"] == "bench_b"
            assert arm["base_pose"] == expected_base
        finally:
            station.stop()
        assert not (work / ".simulator-arms.json").exists()

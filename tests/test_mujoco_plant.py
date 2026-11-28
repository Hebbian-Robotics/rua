import importlib
import socket
from pathlib import Path

import pytest

from hebbian_rua.simulator import PlantConnection, SimStep


def test_unpowered_actuators_release_and_powered_commands_restore_torque(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    mujoco = pytest.importorskip("mujoco")
    monkeypatch.syspath_prepend(
        str(Path(__file__).resolve().parents[1] / "examples/mujoco_sim")
    )
    plant = importlib.import_module("sim_plant")
    model = mujoco.MjModel.from_xml_string("""
        <mujoco>
          <worldbody>
            <body pos="0 0 1">
              <joint name="arm" type="slide" axis="0 0 1"/>
              <geom type="sphere" size="0.1" mass="1"/>
            </body>
            <body pos="1 0 1">
              <joint name="gripper" type="slide" axis="0 0 1"/>
              <geom type="sphere" size="0.1" mass="1"/>
            </body>
          </worldbody>
          <actuator>
            <position name="arm_servo" joint="arm" kp="100" kv="5"/>
            <position name="gripper_servo" joint="gripper" kp="100" kv="5"/>
          </actuator>
        </mujoco>
    """)
    data = mujoco.MjData(model)
    simulator_socket, station_socket = socket.socketpair()
    with station_socket, PlantConnection(simulator_socket) as transport:
        connection = plant._open(
            model,
            data,
            "arm",
            plant.PlantArm(
                ["arm"],
                ["arm_servo"],
                gripper_joint="gripper",
                gripper_actuator="gripper_servo",
                command_delay_seconds=0.01,
            ),
            transport,
        )
        plant._apply(
            data, connection, SimStep(positions=[0.0], powered=False, seconds=0.1), 0.0
        )
        for _ in range(50):
            mujoco.mj_step(model, data)
        assert all(position < -0.01 for position in data.qpos)
        assert all(force == 0 for force in data.actuator_force)

        plant._apply(
            data,
            connection,
            SimStep(positions=[0.0], gripper_angle=0.0, powered=True, seconds=0.1),
            0.1,
        )
        plant._deliver(data, connection, 0.1)
        mujoco.mj_forward(model, data)
        assert all(force == 0 for force in data.actuator_force)
        plant._deliver(data, connection, 0.11)
        mujoco.mj_forward(model, data)
        assert all(force > 0 for force in data.actuator_force)

        plant._apply(
            data,
            connection,
            SimStep(positions=[0.0], gripper_angle=0.0, powered=True, seconds=0.1),
            0.12,
        )
        plant._apply(
            data,
            connection,
            SimStep(positions=[0.0], powered=False, seconds=0.1),
            0.125,
        )
        plant._deliver(data, connection, 0.14)
        mujoco.mj_forward(model, data)
        assert all(force == 0 for force in data.actuator_force)

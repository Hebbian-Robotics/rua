from collections import deque
from collections.abc import Callable, Mapping
from contextlib import ExitStack
from dataclasses import dataclass
from pathlib import Path

from mujoco_runtime import mujoco

from hebbian_rua.simulator import PlantConnection, PlantDisconnected, SimState, SimStep


@dataclass(frozen=True)
class PlantArm:
    joint_names: list[str]
    actuator_names: list[str]
    gripper_joint: str | None = None
    # The bus round trip: each commanded target reaches the actuators this long after
    # its step arrives. The actuators' own gains carry the servos' following lag.
    # Zero applies the command at once.
    command_delay_seconds: float = 0.0
    gripper_actuator: str | None = None
    # Scale MuJoCo's actuator effort to the units expected by the station.
    effort_per_newton_metre: float = 1.0
    # The robot description's names for the same joints (the arm's, then the
    # gripper's), which the executor drives by. Give them when your scene names
    # differ from the URDF's, as with a prefix per arm; defaults to the scene names.
    executor_joint_names: list[str] | None = None


@dataclass
class _Connection:
    transport: PlantConnection
    joint_addresses: list[int]
    velocity_addresses: list[int]
    actuator_ids: list[int]
    gripper_address: int | None
    gripper_actuator_id: int | None
    effort_per_newton_metre: float
    joint_names: list[str]
    command_delay_seconds: float
    # Commanded targets not yet delivered, with the simulated time each is due.
    pending: deque[tuple[float, list[float]]]


def _state(mujoco_data: mujoco.MjData, connection: _Connection) -> SimState:
    gripper_angle = (
        None
        if connection.gripper_address is None
        else float(mujoco_data.qpos[connection.gripper_address])
    )
    return SimState(
        positions=[float(mujoco_data.qpos[a]) for a in connection.joint_addresses],
        velocities=[float(mujoco_data.qvel[a]) for a in connection.velocity_addresses],
        efforts=[
            abs(float(mujoco_data.actuator_force[a]))
            * connection.effort_per_newton_metre
            for a in connection.actuator_ids
        ],
        gripper_angle=gripper_angle,
    )


def _open(
    model: mujoco.MjModel,
    data: mujoco.MjData,
    name: str,
    arm: PlantArm,
    transport: PlantConnection,
) -> _Connection:
    if (arm.gripper_joint is None) != (arm.gripper_actuator is None):
        raise ValueError(f"{name}: a gripper needs both its joint and its actuator")
    gripper_address = (
        None
        if arm.gripper_joint is None
        else int(model.joint(arm.gripper_joint).qposadr[0])
    )
    gripper_actuator_id = (
        None
        if arm.gripper_actuator is None
        else int(model.actuator(arm.gripper_actuator).id)
    )
    connection = _Connection(
        transport=transport,
        joint_addresses=[
            int(model.joint(joint).qposadr[0]) for joint in arm.joint_names
        ],
        velocity_addresses=[
            int(model.joint(joint).dofadr[0]) for joint in arm.joint_names
        ],
        actuator_ids=[
            int(model.actuator(actuator).id) for actuator in arm.actuator_names
        ],
        gripper_address=gripper_address,
        gripper_actuator_id=gripper_actuator_id,
        effort_per_newton_metre=arm.effort_per_newton_metre,
        command_delay_seconds=arm.command_delay_seconds,
        pending=deque(),
        joint_names=arm.executor_joint_names
        or [
            *arm.joint_names,
            *([] if arm.gripper_joint is None else [arm.gripper_joint]),
        ],
    )
    transport.send_hello(connection.joint_names, _state(data, connection))
    return connection


def serve_plants(
    model: mujoco.MjModel,
    data: mujoco.MjData,
    arms: Mapping[str, PlantArm],
    socket_directory: Path,
    *,
    after_step: Callable[[], None] | None = None,
    connect_timeout_seconds: float = 60.0,
) -> None:

    with ExitStack() as stack:
        connections = [
            _open(
                model,
                data,
                name,
                arm,
                stack.enter_context(
                    PlantConnection.connect(
                        socket_directory, name, timeout_seconds=connect_timeout_seconds
                    )
                ),
            )
            for name, arm in arms.items()
        ]
        simulated_seconds = 0.0
        try:
            while True:
                steps = [connection.transport.read_step() for connection in connections]
                for connection, step in zip(connections, steps, strict=True):
                    _apply(data, connection, step, simulated_seconds)
                substeps = max(1, round(steps[0].seconds / model.opt.timestep))
                for _ in range(substeps):
                    for connection in connections:
                        _deliver(data, connection, simulated_seconds)
                    mujoco.mj_step(model, data)
                    simulated_seconds += model.opt.timestep
                if after_step is not None:
                    after_step()
                for connection in connections:
                    connection.transport.send_state(_state(data, connection))
        except (PlantDisconnected, BrokenPipeError, ConnectionResetError):
            return


def _apply(
    data: mujoco.MjData, connection: _Connection, step: SimStep, now: float
) -> None:

    if len(step.positions) != len(connection.actuator_ids):
        raise ValueError(
            f"Step has {len(step.positions)} targets for {len(connection.actuator_ids)} actuators"
        )
    if step.powered:
        connection.pending.append(
            (now + connection.command_delay_seconds, list(step.positions))
        )
    else:
        connection.pending.clear()
        for actuator, address in zip(
            connection.actuator_ids, connection.joint_addresses, strict=True
        ):
            data.ctrl[actuator] = data.qpos[address]
    if connection.gripper_actuator_id is not None:
        assert connection.gripper_address is not None
        if step.powered and step.gripper_angle is not None:
            data.ctrl[connection.gripper_actuator_id] = step.gripper_angle
        else:
            data.ctrl[connection.gripper_actuator_id] = data.qpos[
                connection.gripper_address
            ]


def _deliver(data: mujoco.MjData, connection: _Connection, now: float) -> None:
    # WHY: A nanosecond of slack keeps float rounding in the clock from holding a due target back.
    while connection.pending and connection.pending[0][0] <= now + 1e-9:
        _, targets = connection.pending.popleft()
        for actuator, target in zip(connection.actuator_ids, targets, strict=True):
            data.ctrl[actuator] = target

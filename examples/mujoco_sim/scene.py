import math

import numpy as np
from arm_config import ArmConfig
from mujoco_runtime import mujoco
from settings import MujocoSettings
from so101_model import simulated_arm_model

FRAME_WIDTH = 640
FRAME_HEIGHT = 480

# A wrist camera on each arm's camera board, the model's nominal mounting frame: +Z toward
# the scene, +Y toward the connector edge nearest the gripper. MuJoCo cameras look along
# local -Z, so the camera turns half a turn about X to look out of the board's front face.
WRIST_CAMERA_NAME = "wrist"
WRIST_CAMERA_POSITION = (0.0, 0.0, 0.016)
WRIST_CAMERA_QUATERNION = (0.0, 1.0, 0.0, 0.0)
WRIST_CAMERA_FIELD_OF_VIEW_DEGREES = 75.0


# Keep simulated saturation below the trip threshold, including float32 rounding.
SIMULATED_EFFORT_HEADROOM = 0.99


def scene_name(arm: str, name: str) -> str:
    return f"{arm}_{name}"


def command_delay_seconds(model: mujoco.MjModel, arm: str) -> float:
    # WHY: The arm model carries the command delay fitted together with its servo response.
    numeric = model.numeric(scene_name(arm, "command_delay_seconds"))  # ty: ignore[missing-attribute]  # Missing from the mujoco stubs.
    return float(numeric.data[0])


def apply_profile_effort_limits(
    model: mujoco.MjModel,
    actuator_ids: list[int],
    config: ArmConfig,
    settings: MujocoSettings,
) -> None:

    limits = np.asarray(config.joint_effort_limits, dtype=float)
    effort_scale = config.simulation_model.effort_per_newton_metre(
        settings.rated_actuator_force
    )
    limits *= SIMULATED_EFFORT_HEADROOM / effort_scale
    for actuator_id, limit in zip(actuator_ids, limits, strict=True):
        lower, upper = -float(limit), float(limit)
        if model.actuator_forcelimited[actuator_id]:
            lower = max(lower, float(model.actuator_forcerange[actuator_id, 0]))
            upper = min(upper, float(model.actuator_forcerange[actuator_id, 1]))
        if lower >= upper:
            raise ValueError(
                "Effort budget conflicts with the actuator's existing force range"
            )
        model.actuator_forcelimited[actuator_id] = True
        model.actuator_forcerange[actuator_id] = [lower, upper]


def compile_scene(
    configs: dict[str, ArmConfig],
    settings: MujocoSettings,
) -> mujoco.MjModel:
    spec = mujoco.MjSpec.from_file(str(settings.environment))
    for arm, config in configs.items():
        arm_model, camera_board_frame = simulated_arm_model(config.urdf)
        base = config.base_pose
        position = [0.0, 0.0, 0.0] if base is None else base.position_meters
        yaw = 0.0 if base is None else base.yaw_radians
        frame = spec.worldbody.add_frame(
            pos=position, quat=[math.cos(yaw / 2), 0.0, 0.0, math.sin(yaw / 2)]
        )
        arm_spec = mujoco.MjSpec.from_file(str(arm_model))
        arm_spec.body(camera_board_frame).add_camera(
            name=WRIST_CAMERA_NAME,
            pos=WRIST_CAMERA_POSITION,
            quat=WRIST_CAMERA_QUATERNION,
            fovy=WRIST_CAMERA_FIELD_OF_VIEW_DEGREES,
        )
        # WHY: Attaching keeps the scene's solver options; the arm model's own (elliptic cones
        # and impedance ratio) come from the fitted model. The settings then set
        # the timestep and integrator.
        for option in SOLVER_OPTIONS_FROM_ARM_MODEL:
            setattr(spec.option, option, getattr(arm_spec.option, option))  # ty: ignore[missing-attribute]  # Missing from the mujoco stubs.
        apply_joint_stops(
            arm_spec,
            config.station.joint_names,
            config.lower_limits,
            config.upper_limits,
        )
        frame.attach_body(arm_spec.worldbody.first_body(), scene_name(arm, ""), "")
    model = spec.compile()
    model.opt.timestep = settings.timestep_seconds
    model.opt.integrator = {
        "euler": mujoco.mjtIntegrator.mjINT_EULER,
        "implicitfast": mujoco.mjtIntegrator.mjINT_IMPLICITFAST,
    }[settings.integrator]
    return model


def overview_camera(configs: dict[str, ArmConfig]) -> mujoco.MjvCamera:

    base_positions = [
        config.base_pose.position_meters if config.base_pose else (0.0, 0.0, 0.0)
        for config in configs.values()
    ]
    centre = [
        sum(axis) / len(base_positions) for axis in zip(*base_positions, strict=True)
    ]
    spread = max(math.dist(position, centre) for position in base_positions)
    camera = mujoco.MjvCamera()
    camera.lookat[:] = np.add((0.15, 0.0, 0.1), centre)
    camera.distance = 0.75 + 2 * spread
    camera.azimuth = 135
    camera.elevation = -25
    return camera


SOLVER_OPTIONS_FROM_ARM_MODEL = (
    "cone",
    "impratio",
    "iterations",
    "ls_iterations",
    "timestep",
    "integrator",
)


def apply_joint_stops(
    arm_spec: mujoco.MjSpec,
    joint_names: list[str],
    lower: list[float],
    upper: list[float],
) -> None:
    for name, low, high in zip(joint_names, lower, upper, strict=True):
        joint = arm_spec.joint(name)  # ty: ignore[missing-attribute]  # Missing from the mujoco stubs.
        joint.limited = mujoco.mjtLimited.mjLIMITED_TRUE  # ty: ignore[missing-attribute]  # Missing from the mujoco stubs.
        joint.range = [low, high]
        actuator = arm_spec.actuator(name)  # ty: ignore[missing-attribute]  # Missing from the mujoco stubs.
        actuator.ctrllimited = mujoco.mjtLimited.mjLIMITED_TRUE  # ty: ignore[missing-attribute]  # Missing from the mujoco stubs.
        actuator.ctrlrange = [low, high]


def build_scene(
    configs: dict[str, ArmConfig],
    settings: MujocoSettings,
) -> tuple[mujoco.MjModel, mujoco.MjData]:
    model = compile_scene(configs, settings)
    data = mujoco.MjData(model)
    for arm, config in configs.items():
        joint_names = config.station.joint_names
        start_pose = config.simulation_model.initial_joint_positions_radians
        actuator_ids = [
            int(model.actuator(scene_name(arm, name)).id) for name in joint_names
        ]
        apply_profile_effort_limits(model, actuator_ids, config, settings)
        for name, angle in zip(
            joint_names,
            settings.start_pose_radians if start_pose is None else start_pose,
            strict=True,
        ):
            joint = model.joint(scene_name(arm, name))
            # WHY: The physical rest reference can extend beyond this arm's simulated stops.
            if model.jnt_limited[joint.id]:
                lower, upper = model.jnt_range[joint.id]
                angle = float(np.clip(angle, lower, upper))
            data.qpos[int(joint.qposadr[0])] = angle
            data.ctrl[int(model.actuator(scene_name(arm, name)).id)] = angle
        gripper_force_fraction = config.simulation_model.gripper_force_fraction
        if gripper_force_fraction is not None:
            # The servo's torque limit caps the grip, as on the hardware.
            gripper = int(model.actuator(scene_name(arm, settings.gripper_joint)).id)
            model.actuator_forcerange[gripper] *= gripper_force_fraction
            address = int(
                model.joint(scene_name(arm, settings.gripper_joint)).qposadr[0]
            )
            data.ctrl[gripper] = data.qpos[address]
    mujoco.mj_forward(model, data)
    return model, data

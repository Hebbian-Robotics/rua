import io
from pathlib import Path

import cyclopts
from arm_config import load_station_arms
from camera_server import CAMERA_SOCKET_NAME, CameraServer
from mujoco_runtime import mujoco
from PIL import Image
from scene import (
    FRAME_HEIGHT,
    FRAME_WIDTH,
    WRIST_CAMERA_NAME,
    build_scene,
    command_delay_seconds,
    overview_camera,
    scene_name,
)
from settings import MujocoSettings
from sim_plant import PlantArm, serve_plants

CLI_HELP = """Development fixture for plant transport, control and camera validation.

Start the station with a simulation profile, then run this fixture with the
same socket directory. It supplies one overview and each arm's
wrist camera. Users own their simulator; see docs/simulators.md for the public
adapter contract.
"""

command_line = cyclopts.App(help=CLI_HELP)


@command_line.default
def main(socket_directory: Path, *, timestep_seconds: float | None = None) -> None:
    configs = load_station_arms(socket_directory)
    settings = (
        MujocoSettings()
        if timestep_seconds is None
        else MujocoSettings(timestep_seconds=timestep_seconds)
    )
    model, data = build_scene(configs, settings)
    renderer: mujoco.Renderer | None = None

    # Each arm's wrist camera rides on its gripper: "wrist" for one arm,
    # "<arm>_wrist" for several. The scene names it "<arm>_wrist".
    wrist_cameras = {
        WRIST_CAMERA_NAME
        if len(configs) == 1
        else scene_name(arm, WRIST_CAMERA_NAME): scene_name(arm, WRIST_CAMERA_NAME)
        for arm in configs
    }

    camera_views: dict[str, mujoco.MjvCamera | str] = {
        "overview": overview_camera(configs),
        **wrist_cameras,
    }

    def render(camera: str) -> bytes:
        nonlocal renderer
        # Created on first use, on this thread, which owns the GL context.
        if renderer is None:
            renderer = mujoco.Renderer(model, FRAME_HEIGHT, FRAME_WIDTH)
        renderer.update_scene(data, camera=camera_views[camera])
        encoded = io.BytesIO()
        Image.fromarray(renderer.render()).save(encoded, format="JPEG", quality=85)
        return encoded.getvalue()

    cameras = CameraServer(
        socket_directory / CAMERA_SOCKET_NAME,
        sorted(camera_views),
        render,
    )
    arms = {}
    for arm, config in configs.items():
        scene_joints = [scene_name(arm, name) for name in config.station.joint_names]
        gripper = (
            scene_name(arm, settings.gripper_joint)
            if config.simulation_model.gripper_force_fraction is not None
            else None
        )
        arms[arm] = PlantArm(
            joint_names=scene_joints,
            controller_joint_names=config.driven_joints,
            actuator_names=scene_joints,
            gripper_joint=gripper,
            gripper_actuator=gripper,
            effort_per_newton_metre=config.simulation_model.effort_per_newton_metre(
                settings.rated_actuator_force
            ),
            command_delay_seconds=command_delay_seconds(model, arm),
        )
    try:
        serve_plants(
            model, data, arms, socket_directory, after_step=cameras.serve_pending
        )
    finally:
        cameras.close()
        if renderer is not None:
            renderer.close()


if __name__ == "__main__":
    command_line()

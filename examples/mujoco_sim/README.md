# Development simulator fixture

This source-only MuJoCo fixture validates plant transport, control and camera
integration without hardware. It is not a supported simulation product or task
suite. Users choose their own engine and implement the documented
[simulator adapter contract](../../docs/simulators.md).

The fixture simulates the SO-101 with the TPU gripper and wrist camera, using
the MuJoCo model generated in [Hebbian-Robotics/so101](https://github.com/Hebbian-Robotics/so101)
from the same CAD as the station's `robot-soft-gripper-camera.urdf`. The fixture
reads the assembly manifest prepared by `uv run rua-dev setup` and uses the
commit pinned in `robots/so101-assets.toml`. If the pinned MJCF is missing
locally, the fixture fetches it with a sparse Git checkout in
`.cache/so101-mujoco-model/<commit>/`. The fixture refuses a profile configured
with another URDF. It reads each selected arm's
base pose, rest pose, joint and effort limits, and gripper force fraction from
the station's resolved config. The pinned MJCF supplies the fitted servo
response and command delay. The fixture steps all arms together and provides
one overview camera and one wrist camera per arm. Cameras render lazily on the simulation thread.
There are no task objects, grasping scenarios or extra presentation views.
The fixture inherits the shared SO-101 controller defaults, including motion,
settling, and the gripper force fraction. The plant's scene and physics
settings live in `settings.py` and the pinned MJCF.
Use `--timestep-seconds 0.002` for a 2 ms stress run.

## Run for development

This is an independent uv project using the checkout's model assets. Prepare it before starting the station:

```sh
uv sync --project examples/mujoco_sim
```

Follow the [development simulation guide](../../docs/simulation.md) to start the
station, then connect the fixture using the station work directory:

```sh
uv run --project examples/mujoco_sim python examples/mujoco_sim/run.py --socket-directory /tmp/hebbian-station
```

`sim_plant.py` maps the shared simulator connection to MuJoCo; `scene.py` assembles the
profile's model; `camera_server.py` serves JPEG requests to the media companion.
Physics steps deliver delayed targets individually, so batching them with `nstep`
would change the behavior being exercised. The fixture approximates actuator behavior;
it does not establish hardware tracking, contact or grasping accuracy.

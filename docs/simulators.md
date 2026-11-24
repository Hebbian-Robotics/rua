# Connect your simulator

RUA controls a robot model. You choose, install and run the physics engine.
No simulator, scene or renderer is included in `hebbian-rua`.

Use [a real arm](getting-started.md) whenever available. Reserve a simulator
profile for testing without hardware.

Start the station with the bundled socket profile, or a profile for your robot:

```sh
uv run rua serve --profile so101-sim-only-no-real-arm --work-directory /tmp/rua-station
```

Then start your simulator integration before the station's connection timeout,
30 seconds by default (`--simulator-connect-seconds`).
For the bundled SO-101 profile it connects to `/tmp/rua-station/arm.plant.sock`.
The `so101-sim-only-no-real-arm` preset and the hardware profile share the
SO-101 control defaults in [`so101-common.toml`](../profiles/so101-common.toml).
The simulator adapter supplies plant feedback and uses its own physics and transport settings.
Each arm has its own Unix-domain stream socket, named `<arm>.plant.sock`.
Arm names in profiles must be unique and use ASCII letters, digits,
underscores or hyphens.
Use a separate work directory for each running station. The socket transport
runs on Unix-like systems. On Windows, run both processes inside WSL2.

For two simulated arms, create a local profile with
`extends = ["so101-sim-only-no-real-arm", "so101-bimanual-layout"]` and give it a `name`.
Override the base poses to match your scene.

## Adapter contract

The installed Python package provides `hebbian_rua.simulator.PlantConnection`
and the `SimState` and `SimStep` message models. Connect once per arm, call
`send_hello(joint_names, state)`, then repeatedly call `read_step()`, advance
your engine, and call `send_state(state)`. Use the connection as a context
manager to close it when your simulator stops. For a shared multi-arm scene,
read every arm's step before advancing physics and send each arm's state after.

Other languages can use UTF-8 JSON, one object per line. RUA listens; your
integration connects.
Export the exact message schema with:

```sh
uv run rua schema --sim-plant > sim-plant.schema.json
```

The schema is also included under `schemas/` in the source checkout and as
`sim-plant.schema.json` in the installed `hebbian_rua` module.

1. Send a hello with `protocol_version: 1`, `joint_names` and initial `state`.
   Joint names must match the configured driven joints, including the gripper
   when present. For SO-101: `shoulder_pan`, `shoulder_lift`, `elbow_flex`,
   `wrist_flex`, `wrist_roll`, `gripper`.
2. Read a step containing arm target `positions`, `gripper_angle`, `seconds`
   and `powered`. Apply these targets to your actuators and advance your engine
   by `seconds`. With `powered: false`, do not hold the commanded targets.
3. Reply with measured `positions`, `velocities`, `efforts` and `gripper_angle`.
   Repeat for each step. Arm vectors exclude the gripper, whose angle is separate.

Angles are radians, velocity is radians/second, time is seconds. Effort units
come from the profile. Feedback must contain finite values with the configured
vector sizes. Disconnects and reply timeouts stop control.

Your adapter owns physics, actuator modeling, contacts, scenes and rendering.
RUA owns target generation and its configured control limits. For cameras,
publish your simulator's frames to LiveKit and use the [camera integration](cameras.md).

For another robot, supply its URDF, joint order, limits and control profile.
Use `driver = "simulator"` without a `[feetech]` section. Keep engine-specific
settings in your simulator application.

## Using an existing MuJoCo simulation

Keep your MJCF, scene setup and simulation process. Use the shared Python
connection above and add engine-specific code: map the configured
joint names to your model, set actuator targets from each step, advance MuJoCo
for the requested duration, and return measured joint state. Use physics substeps
when your model's timestep is shorter than the requested duration. In a shared
multi-arm scene, collect every arm's targets before advancing physics once for
that duration, then reply to each arm.

Start RUA with a simulator profile and run your application separately. Your
application owns actuator gains, initial pose, rendering and its run loop.
The [MuJoCo quickstart](simulation.md) shows how the source example uses the
shared connection. The wheel provides the same Python connection API.

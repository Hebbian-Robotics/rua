# Connect your simulator

RUA controls a robot model. You choose, install and run the physics engine.
No simulator, scene or renderer is included in `hebbian-rua`.

Start the station with the bundled socket profile, or a profile for your robot:

```sh
uv run rua serve --profile so101-socket --work-directory /tmp/rua-station
```

Then start your simulator adapter before the station's connection timeout.
For the bundled SO-101 profile it connects to `/tmp/rua-station/follower.plant.sock`.
Each arm has its own Unix-domain stream socket, named `<arm>.plant.sock`.
This transport currently requires a Unix-like OS; Windows is not supported.

## Adapter contract

Use UTF-8 JSON, one object per line. RUA listens; your adapter connects.
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
Simulator profiles use `driver = "simulator"` with no `[feetech]` section;
engine settings such as model files and timesteps belong to your adapter.

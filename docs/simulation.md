# Simulation

## Run the MuJoCo example

Run a simulated SO-101 arm and control it with the Python client. To connect a
different engine, use the [simulator interface](simulators.md).

### Install

For this source-only example, follow the
[source installation guide](install-from-source.md), then return here. The
quick start has been exercised on Linux x86-64; see that guide for platform
status and prerequisites.

Prepare the separate example environments before starting the station:

```sh
uv sync --project examples/mujoco_sim --locked
uv sync --project examples/python --locked
```

These projects use local SDK and tools packages without installing the root
development dependencies into their environments.

### 1. Run a simulated station

From the repository root, start the station in one terminal:

```sh
uv run cargo run --locked -p hebbian-robot-use-server -- serve --profile "$PWD/examples/mujoco_sim/profiles/single.toml" --work-directory /tmp/hebbian-station
```

When the station prints `Plant sockets`, start the simulator from the
repository root in a second terminal. The default connection timeout is
30 seconds per arm. The first run may need to download the
pinned MJCF model. Set `--simulator-connect-seconds` on the station to give it more time.

```sh
uv run --project examples/mujoco_sim --locked python examples/mujoco_sim/run.py --socket-directory /tmp/hebbian-station
```

The fixture serves JPEG snapshots for `sim_overview` and `sim_wrist`, with
one named wrist view per arm in multi-arm profiles. Build the
[media companion](cameras.md#rua-configuration) to give SDK clients access to
these snapshots. See the [simulation reference](reference.md#so-101-in-mujoco)
for fixture limitations.

For two arms or learned control, start the station with
[bimanual.toml](../examples/mujoco_sim/profiles/bimanual.toml) or
[policy.toml](../examples/mujoco_sim/profiles/policy.toml). The simulator command
is the same.

### 2. Run hello_robot

Wait until [station state](http://127.0.0.1:8792/state) shows populated
`measured_joints` for the simulated arm. HTTP can come up while the controller
is still waiting for feedback. Then, from the repository root in a third
terminal, run the example
([`examples/python/hello_robot.py`](../examples/python/hello_robot.py) enables the arm itself):

```sh
uv run --project examples/python --locked python examples/python/hello_robot.py
```

The script previews a target 160 mm above the fixture's table at
`[0.18, 0.0, 0.16]` metres. After the move completes without a limit, it opens
the gripper and prints the measured tool position.

## Optional: open the viewer

<http://127.0.0.1:8792/viewer/> shows the arm. To show simulated camera video in
the viewer or record it, publish the feeds to the station's [LiveKit room](cameras.md).
The bundled fixture supplies snapshots only. Add
`--manual-control` to the server command for sliders that drive the arm (don't
use them while a program holds control). Build it once with
`pnpm install --frozen-lockfile && pnpm build`; `pnpm dev` serves it with hot
reload against a running station.

## Optional: let a model drive

Needs `OPENAI_API_KEY` (exported or in `.env`) and the media service built
as described above for the requested camera.
The [TypeScript agent](../examples/typescript/agent/README.md) needs the arm
enabled first. Enable it through the viewer, or install the CLI with
`cargo install --path cli` and run `hebbian-robot-use enable arm`.
Pass `--camera sim_overview` if you want it to see the simulated camera.
The Python OpenAI loop enables the arm itself:

```sh
# Set OPENAI_API_KEY and set OPENAI_MODEL to a model ID from your account.
uv run --project examples/python --locked python examples/python/agent_quickstart.py "Wave hello" --model "$OPENAI_MODEL" --camera sim_overview
```

To use another SDK, keyboard, a gamepad or vision, see
[write your own loop](write-your-own-loop.md). Every runnable example is indexed in
[examples/README.md](../examples/README.md).

## Next steps

- [Write your own loop](write-your-own-loop.md) using the client library or a model.
- [Set up an SO-101](so101-manual-control.md#set-up-each-so-101) before moving
  a physical arm. Support the arm when disabling it, check its calibration with
  torque off, and keep a physical power cutoff within reach.
- [Connect cameras](cameras.md), or enable
  [recording](recording.md) when you need it.
- Explore the [examples](../examples/README.md) and [documentation index](README.md).

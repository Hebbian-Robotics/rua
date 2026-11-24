# Simulate the SO-101 with MuJoCo

Run the same station and SDK flow against a virtual SO-101. The fixture models
the arm and table. For physics and model details, see the
[MuJoCo fixture](../examples/mujoco_sim/README.md). To connect a different engine,
use the [simulator interface](simulators.md).

## Prepare the checkout

Follow the [source setup guide](install-from-source.md#prepare-the-checkout).
From the repository root, install the example dependencies:

```sh
uv sync --project examples/mujoco_sim --locked
uv sync --project examples/python --locked
```

## 1. Run a simulated station

Start the station from the repository root:

```sh
uv run cargo run --locked -p hebbian-rua-server -- serve \
  --profile "$PWD/examples/mujoco_sim/profiles/single.toml" \
  --work-directory /tmp/hebbian-station
```

When the station prints `Plant sockets`, start MuJoCo in a second terminal:

```sh
uv run --project examples/mujoco_sim --locked python examples/mujoco_sim/run.py \
  --socket-directory /tmp/hebbian-station
```

The first run may download the pinned MJCF model. If the download needs more
than the default 30 seconds, add `--simulator-connect-seconds 120` to the
station command.

## 2. Run hello_robot

Wait for populated `measured_joints` in [station state](http://127.0.0.1:8792/state)
or run `cargo run --locked -p hebbian-rua-cli -- state`. Then run the
[Python example](../examples/python/hello_robot.py) in a third terminal:

```sh
uv run --project examples/python --locked python examples/python/hello_robot.py
```

The example enables the arm, previews a target 160 mm above the table, moves
to it, and opens the gripper after the move completes.

For two arms, restart the station with
[`bimanual.toml`](../examples/mujoco_sim/profiles/bimanual.toml) in place of
`single.toml` and use the same MuJoCo command. Pass `--arm left` or
`--arm right` to `hello_robot.py` to select one.

## Run an ONNX policy

The [policy profile](../examples/mujoco_sim/profiles/policy.toml) selects the
single-arm fixture and the [SO-101 policy adapter](hardware.md#learned-policies).
Supply a compatible ONNX model and an ONNX Runtime shared library. Create a
local profile that extends the example and points at your model:

```sh
policy_directory="$(mktemp -d)"
cat > "$policy_directory/policy.toml" <<EOF
extends = ["$PWD/examples/mujoco_sim/profiles/policy.toml"]

[policy]
onnx = "/absolute/path/to/policy.onnx"
EOF

ORT_DYLIB_PATH=/absolute/path/to/libonnxruntime.so \
  uv run cargo run --locked -p hebbian-rua-server -- serve \
  --profile "$policy_directory/policy.toml" \
  --profile-directory "$PWD/profiles" \
  --work-directory /tmp/hebbian-station
```

Use the library for your platform, such as `libonnxruntime.dylib` on macOS.
Start MuJoCo in the second terminal as above. Use pose goals to exercise the
policy adapter. The example profile supplies controller settings; you supply the
model.

## Camera snapshots

Build the [media companion](cameras.md#rua-configuration) before starting the
station. For headless rendering on Linux, install OSMesa (`libosmesa6` on
Ubuntu) or set `MUJOCO_GL` to a backend supported by your system.

```sh
cargo run --locked -p hebbian-rua-cli -- camera list
cargo run --locked -p hebbian-rua-cli -- camera snapshot sim_overview --out overview.jpg
```

Use `sim_wrist` for a single-arm wrist view or `sim_left_wrist` and
`sim_right_wrist` for the bimanual fixture.

MuJoCo's camera feeds are JPEG snapshots from the simulator socket. The
[LiveKit room viewer](cameras.md#watch-a-usb-camera) shows tracks published to
LiveKit, such as a USB or phone camera.

## Run a model loop

For a single simulated arm, set `OPENAI_API_KEY` in your environment. The
example supplies the model with robot tools and captures images
if you pass a camera name:

```sh
uv run --project examples/python --locked python examples/python/agent_quickstart.py \
  --provider openai --model gpt-6.2-astra "Wave hello" --camera sim_overview
```

Omit `--camera` for a state-only run. See the
[Python agent guide](../examples/python/README.md#run-an-agent) for other
providers and options.

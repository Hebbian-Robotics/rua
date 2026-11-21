# Simulate the SO-101 with MuJoCo

Run a virtual SO-101 and control it over the same station API used for hardware.
The example uses the [simulator adapter](simulators.md) to connect MuJoCo to RUA.

## Prepare the checkout

Follow the [source setup guide](install-from-source.md#prepare-the-checkout).
From the checkout root, install the example dependencies:

```sh
uv sync --project examples/mujoco_sim --locked
uv sync --project examples/python --locked
```

## Start the station and MuJoCo

In one terminal, start the station from the checkout root:

```sh
uv run cargo run --locked -p hebbian-robot-use-server -- serve \
  --profile "$PWD/examples/mujoco_sim/profiles/single.toml" \
  --work-directory /tmp/hebbian-station --simulator-connect-seconds 120
```

When the station prints `Plant sockets`, launch MuJoCo from the same checkout
in a second terminal. The first run downloads the pinned SO-101 MuJoCo model.

```sh
uv run --project examples/mujoco_sim --locked python examples/mujoco_sim/run.py \
  --socket-directory /tmp/hebbian-station
```

## Read state and move the virtual arm

In a third terminal, read the state with the [station CLI](../cli/README.md).
Wait for populated `measured_joints` before sending motion commands.

```sh
cargo run --locked -p hebbian-robot-use-cli -- state
```

Run the Python example to preview a target, move the arm and open the gripper:

```sh
uv run --project examples/python --locked python examples/python/hello_robot.py
```

For two arms, restart the station with
[`bimanual.toml`](../examples/mujoco_sim/profiles/bimanual.toml) in place of
`single.toml` and run the same MuJoCo command. The CLI shows both arms. Pass
`--arm left` or `--arm right` to `hello_robot.py` to select one.

## Camera snapshots

Build the [media companion](cameras.md#rua-configuration) before starting the
station to enable JPEG snapshots. On Linux, install OSMesa (`libosmesa6` on
Ubuntu) for headless rendering, or set `MUJOCO_GL` to a backend for your system.

```sh
cargo run --locked -p hebbian-robot-use-cli -- camera list
cargo run --locked -p hebbian-robot-use-cli -- camera snapshot sim_overview --out overview.jpg
```

Use `sim_wrist` for the single-arm wrist view and `sim_left_wrist` or
`sim_right_wrist` for the two-arm wrist views.

See the [example README](../examples/mujoco_sim/README.md) for model and physics
details.

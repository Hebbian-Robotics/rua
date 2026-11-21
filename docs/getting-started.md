# Get started

## Set up a physical SO-101

Clone this repository and [prepare the source checkout](install-from-source.md#prepare-the-checkout).
Follow the [SO-101 calibration guide](so101-calibration.md) to save the arm's
`setup.toml` and `calibration.json`. From the checkout root, start the station:

```sh
uv run cargo run -p hebbian-robot-use-server -- serve
```

With torque off, move each joint and the gripper by hand and check that the
positions reported in [station state](http://127.0.0.1:8792/state) agree with
the physical arm before enabling it.

If you've already calibrated the arm with LeRobot, you can
[reuse that calibration with an explicit profile](hardware.md).

With the station running, [read your arm's state with the Python SDK](../examples/python/README.md#read-your-arms-state).
Then [run the Python agent](../examples/python/README.md#run-an-agent). To give
it images, [connect a camera](cameras.md#connect-a-usb-camera) and pass its
label to the example.

## Install the published wheel

`hebbian-rua` bundles the Python SDK, Rust server, media companion and robot
models in one wheel. Prebuilt wheels support Linux x86-64 with glibc 2.39+
(such as Ubuntu 24.04) and macOS 14+ on Apple Silicon.
No Rust compiler is needed for those installs.
On Windows, see [Windows with WSL2](windows-wsl.md) for setup considerations.

Use [uv](https://docs.astral.sh/uv/) for your agent project:

```sh
uv init my-robot-agent
cd my-robot-agent
uv add "hebbian-rua[openai,anthropic,gemini]"
```

In an existing uv project, start with `uv add`. This adds RUA to `pyproject.toml`
and resolves its dependencies into `uv.lock`. Run your agent with `uv run` too.
Choose only the provider extras you need, such as `hebbian-rua[anthropic]`.

The published RUA 0.1.2 wheel uses the
[0.1.2 hardware profile format](https://github.com/Hebbian-Robotics/rua/blob/v0.1.2/docs/hardware.md) for LeRobot calibrations. To start
the wheel's station with your profile, pass its path:

```sh
uv run rua serve --profile "$HOME/.config/rua/so101.toml"
```

Use the path where you saved your profile if it differs from the example.

Use `from hebbian_rua import StationClient` in your Python program. See the
[read-only state check](../examples/python/README.md#read-your-arms-state) and
[Python agent example](../examples/python/README.md#run-an-agent).

## Try RUA without hardware

Follow the [MuJoCo quickstart](simulation.md) from a source checkout, or
[connect your own simulator](simulators.md).

## Cameras

For camera setup and the hosted LiveKit viewer, see [cameras](cameras.md).

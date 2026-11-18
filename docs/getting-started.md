# Get started

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

Create your own calibrated profile using [per-arm setup](hardware.md), then start
the station with that file:

```sh
uv run rua serve --profile ./robot.toml
```

`robot.toml` is your profile path, not a generated or bundled configuration.

Use `from hebbian_rua import StationClient` in your Python program. See the
[Python examples](../examples/python/README.md). Physical robots need
[per-arm setup](hardware.md). For simulation, run your own engine and
[connect its adapter](simulators.md); no engine is included or started by RUA.

## From source

Follow [source installation](install-from-source.md), then return here.
For cameras and the web preview, see [cameras](cameras.md).

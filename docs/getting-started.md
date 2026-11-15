# Get started

`hebbian-rua` bundles the Python SDK, Rust server, media companion and robot
models in one wheel. The prebuilt wheel targets Ubuntu 24.04 x86-64
(Linux with glibc 2.39 or newer). No Rust compiler is needed for that install.

Use [uv](https://docs.astral.sh/uv/) for your agent project:

```sh
uv init my-robot-agent
cd my-robot-agent
uv add "hebbian-rua[openai,anthropic,gemini]"
uv run rua serve --profile ./robot.toml
```

In an existing uv project, start with `uv add`. This adds RUA to `pyproject.toml`
and resolves its dependencies into `uv.lock`. Run your agent with `uv run` too.
Choose only the provider extras you need, such as `hebbian-rua[anthropic]`.

Use `from hebbian_rua import StationClient` in your Python program. See the
[Python examples](../examples/python/README.md). Physical robots need
[per-arm setup](hardware.md). For simulation, run your own engine and
[connect its adapter](simulators.md); no engine is included or started by RUA.

## From source

Contributors need Rust 1.92+, Python 3.12+ and uv. On Ubuntu, install
`build-essential`, `pkg-config`, `libudev-dev` and `mold`, then:

```sh
uv run rua-dev setup
cargo build -p hebbian-station
uv sync --project examples/python --locked
uv run --project examples/python rua serve --profile ./robot.toml
```

Source setup prepares the pinned SO-101 models in the ignored `robots/so101`
directory and caches its release archive under `.cache/so101`. Do this once
before building; later builds and runs need no asset download. The installed
wheel already includes the models, license and attribution.

For cameras and the web preview, see [cameras](cameras.md).

# Install from source

The published `hebbian-rua` wheel includes the SDK, native binaries and robot
models. Source development additionally needs Rust, Python and uv. This workflow
is exercised on Ubuntu 24.04 x86-64; macOS and Windows are not validated for RUA.

Install [uv](https://docs.astral.sh/uv/), the pinned Rust toolchain and, on Ubuntu,
`build-essential`, `pkg-config`, `libudev-dev` and `mold`. From the checkout root:

```sh
uv run rua-dev setup
cargo build -p hebbian-station
uv sync --project examples/python --locked
```

Setup prepares the checksum-pinned SO-101 models in ignored `robots/so101` and
caches their release archive under `.cache/so101`. Once prepared, model assets
need no network access for builds or runs. Installed wheels need no setup step.

Run the station with your calibrated profile or simulator connected:

```sh
uv run rua serve --profile ./robot.toml
```

See [hardware setup](hardware.md), [simulator integration](simulators.md) and
[cameras](cameras.md) for the RUA connection steps. `uv run rua-dev --help` lists
checks and commands for the components included in your checkout.

## Optional viewer

If your checkout includes the TypeScript viewer, install Node.js and pnpm, then:

```sh
pnpm install --frozen-lockfile
pnpm build
```

The station serves the resulting viewer assets from disk. Camera builds have
additional [native prerequisites](cameras.md#rua-configuration); recording needs
[FFmpeg](recording.md). Building the Rust server alone needs neither frontend
nor media build tools.

# Install from source

The published `hebbian-rua` wheel includes the SDK, native binaries and robot
models. Source development additionally needs Rust, Python and uv. This workflow
is exercised on Ubuntu 24.04 x86-64 and macOS 14+ (Apple Silicon).
Native Windows is not validated for RUA.
For Windows setup considerations, see [Windows with WSL2](windows-wsl.md).

## Prepare the checkout

Install [uv](https://docs.astral.sh/uv/), the pinned Rust toolchain and, on Ubuntu,
`build-essential`, `pkg-config`, `libudev-dev` and `mold`. On macOS, install
Xcode Command Line Tools (`xcode-select --install`) and use the default Apple
Clang compiler. This checkout pins Python in `.python-version` (uv can install
it) and Rust in `rust-toolchain.toml` (rustup selects it). From the checkout root:

```sh
uv run rua-dev setup
```

Setup prepares the checksum-pinned SO-101 models in ignored `robots/so101` and
caches their pinned Git snapshot under `.cache/so101`. Once prepared, model assets
need no network access for builds or runs. Installed wheels need no setup step.
Sync the separate [Python example environment](../examples/python/README.md)
when you want to run those examples.

## Start the station

For a physical SO-101, [calibrate your arm](so101-calibration.md), then start the
station from the checkout root:

```sh
uv run cargo run -p hebbian-robot-use-server -- serve
```

This source command checks and rebuilds the station before starting it.
`uv run rua serve` launches an existing binary and does not rebuild it; reserve
that command for installed wheels or explicitly selected builds.

See [simulator integration](simulators.md) and
[cameras](cameras.md) for the RUA connection steps. `uv run rua-dev --help` lists
checks and commands for the components included in your checkout.

## Optional viewer

To build the TypeScript viewer, use the Node.js version in `.nvmrc` and the
pnpm version pinned in `package.json`. From the repository root:

```sh
pnpm install --frozen-lockfile
pnpm build
```

The station serves the resulting viewer assets from disk. Camera builds have
additional [native prerequisites](cameras.md#rua-configuration); recording needs
[FFmpeg](recording.md). Building the Rust server alone needs neither frontend
nor media build tools.

## Updating a checkout

Run commands from the repository root. After pulling dependency changes, run
`uv sync` and `pnpm install --frozen-lockfile` (when using TypeScript).

| What changed | Command | When the change takes effect |
| --- | --- | --- |
| Rust station or hardware profile | `uv run cargo run -p hebbian-robot-use-server -- serve` | Support and disable the arm, then stop the station. This builds and starts the current source. To use a custom profile, add `--profile` with its path. |
| Rust CLI | `cargo run -p hebbian-robot-use-cli -- state` | Each invocation checks and builds the current source. An installed `hebbian-robot-use` stays at its installed version. |
| TypeScript agent | `pnpm start:agent` | Restart the agent server to load example changes. |
| TypeScript agent during development | `pnpm dev:agent` | Watches the agent and restarts on changes. |
| Viewer served by the station | `pnpm build` | Refresh the browser; no station restart is needed. |
| Viewer during development | `pnpm dev` | Viewer hot reload and agent server watch mode. |
| Python examples | Rerun the example command | Example changes load on the next process start. |

Example environments and SDK tests use the SDKs in your checkout. Run `pnpm test:package` to validate the packed
TypeScript SDK outside the workspace. `pnpm build` builds the static viewer in
`dist/viewer` without building Rust binaries or restarting services.
A running process continues using its loaded code until it restarts.

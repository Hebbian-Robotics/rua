# Development reference

Follow [Contributing](../CONTRIBUTING.md) for contribution workflow and language
checks, and [architecture](architecture.md#components) for component ownership. Run the
commands below from the repository root. Build/restart instructions live in
[updating a checkout](install-from-source.md#updating-a-checkout).

## Validation details

Use `uv run rua-dev validate --help` for available lanes. Choose a language or
component lane for a focused change, `fast` for work without a Rust build, and
`full` for changes crossing runtime and SDK boundaries. The
[validation implementation](../tooling/rua_dev/validation.py) owns the exact check
selection, test filters and report paths. The [CLI](../tooling/rua_dev/cli.py)
lists the available lanes.

Validation prepares checksum-pinned robot assets, so an unprepared checkout
needs the release download or a populated cache.

Examples of targeted test invocations:

```sh
cargo test -p hebbian-robot-use-core profile::tests
pnpm --filter hebbian-rua test
```

Pytest runs serially so station scenarios do not compete for control-loop
deadlines. Tests that lack optional policy data or dependencies may skip. See
[test ownership](../tests/README.md).

Validation builds the station once and passes its path through
`HEBBIAN_ROBOT_USE_SERVER`. Standalone pytest builds it lazily through a shared
fixture. When deliberately selecting an existing binary, set that variable to
its absolute path and verify it matches the source under test. Documentation-only
changes use the link checks in Contributing; they do not need runtime tests.

## Generated contracts and SDKs

Edit the owning Rust types or route definitions, then regenerate:

```sh
uv run rua-dev contracts
```

The command selects SDKs present in the checkout. Add `--targets python`
or `--targets python rust` to avoid TypeScript tooling for a limited generation.
A shared wire change still needs every affected SDK regenerated before merging.

| Source of truth | Generated output | Generator |
| --- | --- | --- |
| Rust station contracts and route table in `station/src/openapi.rs` | `schemas/openapi.json`, Python models and TypeScript Zod schemas | `tooling/rua_dev/contracts.py` |
| OpenAPI `StationCommand` | Command wrappers in Python, TypeScript and Rust | `generators/commands.py` |
| OpenAPI routes | HTTP helpers in all selected SDKs | `generators/routes.py` |
| `contracts/src/robot_tools.rs` | `schemas/robot-tools.json`, `schemas/robot-context.json`, Python/TypeScript catalogs | `generators/catalog.py` |
| [`contracts/src/sim_plant.rs`](../contracts/src/sim_plant.rs) | Simulator schema and fixture models when included | `contracts.py` |

Generator paths in the table are relative to `tooling/rua_dev/`. The generators
own output paths; do not maintain parallel lists in package scripts or edit
generated wrappers. Hand-written SDK code owns convenience behavior. The command
generator’s `LEASE_FREE_COMMANDS` determines which methods live on the ordinary
client versus the lease handle; review it when adding a command. Route changes
also need the OpenAPI table updated; `station/tests/openapi.rs` checks agreement
with the routers.

A wire change can also affect hand-written SDK adapters, the CLI, and agents. Check those callers after generation, including lease requirements,
retry identity, revisions and cancellation. Regeneration does not update SDK
convenience behavior or package-root exports. The tool catalog owns robot
capabilities; examples own prompts and application tools.

After reviewing and committing generated changes, verify reproducibility:

```sh
uv run rua-dev contracts --check
```

`--check` **regenerates files**, then compares them with Git HEAD, including
staged differences and untracked outputs. It is not a read-only comparison with
the index. New generated changes must be committed before this check can pass.

## Dependency refreshes

Update only the dependency families in scope. Preserve exact pins, upper bounds,
native-library compatibility and the three-day release delay in uv/pnpm settings.
The pinned Rust toolchain must still build the result. Refresh the independently
resolved environments affected by the change:

```sh
uv lock --upgrade
uv lock --upgrade --project examples/python
uv lock --upgrade --project examples/mujoco_sim
pnpm update --recursive --include-workspace-root --lockfile-only
cargo update
```

These commands refresh compatible resolutions; they do not replace a deliberate
manifest change for an exact version or Git revision. `cargo upgrade` from
cargo-edit can help update compatible manifest requirements; it is an optional
developer tool, not a prerequisite for normal builds. Do not raise Python lower
bounds merely because the lockfile selected a newer version.

| Owned dependency | Pins to inspect when changing that integration |
| --- | --- |
| SO-101 | [Dependency pins](so101-dependency.md) for the runtime and model assets |
| LiveKit Cameras | Git dependencies in `livekit-cameras/Cargo.toml`; keep station access’s `livekit-api` compatible with the pinned LiveKit dependency graph |
| MCAP Tools | Native dependencies in `livekit-cameras/Cargo.toml` |

Validate upstream changes in their owning repository first, then update affected
pins and Cargo/pnpm/uv lockfiles here. When Biome changes, update both checked-in
Biome schema URLs. Optional policy dependencies remain resolved in `uv.lock` even
when their runtimes are not installed; report whether policy execution was tested.

## Build artifacts and releases

See the workspace [Cargo manifest](../Cargo.toml) for build-profile settings.

See [release preparation](releasing.md) for the assembled wheel and publishing.
To build or debug the individual native components, run:

```sh
uv run rua-dev package server --platform local --without-viewer --with-media
uv run rua-dev package server --platform manylinux --without-viewer
uv run rua-dev package media
```

`--with-media` is supported only for local builds.
`package media` archives an already built Ubuntu media wheel. These artifacts
are inputs to the assembled `hebbian-rua` wheel, which is the input accepted by
`package check`.

## Worktrees and tooling

Each worktree has its own `target/`. Optional sccache can share compiled crates;
configure it in your user Cargo settings. Sharing `CARGO_TARGET_DIR` also shares
build locks and lets worktrees invalidate each other’s workspace crates.

## SO-101 reachability sweep

Run this planner benchmark from a checkout with the bundled robot assets.
It generates 30,000 pose requests across self-collision, table clearance with
the open gripper, and a held-out seed with the default closed gripper.

```sh
RUA_REACH_SWEEP_OUTPUT=/tmp/rua-reach.jsonl \
  timeout 180 cargo test -p hebbian-robot-use-core --lib reachability_sweep -- --ignored
```

Each JSON line records the fixture, seed, case index, start joints, reference
joints, target pose, and outcome. The reference joint path must clear the
configured guards for a case to count toward reachability. Planned cases
include endpoint error and planning latency. Compare revisions using the same
fixtures and seeds, with separate Cargo target directories.

Each complete line is a checkpoint. Replay held-out case 3288 with the
half-open range from 3288 to 3289, or omit the end index to resume from a
checkpoint. Use a separate output file for a resumed run.

```sh
RUA_REACH_SWEEP_PROFILE=held_out RUA_REACH_SWEEP_START=3288 RUA_REACH_SWEEP_END=3289 \
  RUA_REACH_SWEEP_OUTPUT=/tmp/rua-replay-3288.jsonl \
  timeout 180 cargo test -p hebbian-robot-use-core --lib reachability_sweep -- --ignored
```

Use the [MuJoCo fixture](simulation.md) to exercise station command outcomes
and measured feedback separately. The fixture stops each joint at the limits the
station writes to `<work-directory>/<arm>.json`: the URDF's, or a registered arm's
calibrated travel with `serve --so101-config-directory CONFIGS --so101-serial SERIAL`.
It simulates the SO-101 with the TPU gripper and wrist camera from the MuJoCo model
generated upstream with `robot-soft-gripper-camera.urdf`, checked out on first run
at the commit pinned in `robots/so101-assets.toml`.
The stand-in with no physics (`--no-physics-stand-in`) takes the same options.

To compare the cost of failed searches, run the companion benchmark. It
measures planner and preview latency on targets beyond the arm's reach and
on positions inside the workspace with an incompatible tool orientation.
Both paths must report that each request falls short.

```sh
RUA_REACH_SWEEP_OUTPUT=/tmp/rua-impossible.jsonl \
  timeout 180 cargo test -p hebbian-robot-use-core --lib impossible_pose_latency -- --ignored
```

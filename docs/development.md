# Development reference

See [Contributing](../CONTRIBUTING.md) for local checks and
[updating a checkout](install-from-source.md#updating-a-checkout) for build and
restart instructions.

## Validation details

The [Check workflow](../.github/workflows/check.yml) runs on Namespace. Pull
requests select checks by changed path. Pushes to `main` and manual runs execute
the full suite. Pull requests can reuse
[Cache Volumes](https://namespace.so/docs/solutions/github-actions/caching), and
only `main` can persist cache updates. The required `integration` check waits
for the station tests and the installed simulator quickstart, which run in
parallel with separate cache volumes. The quickstart installs the assembled wheel
and TypeScript tarball. Its native build disables link-time optimization to
shorten PR feedback. The publish workflow uses the production profiles.

For station integration tests, install JavaScript dependencies with
`pnpm install --frozen-lockfile`, then run `uv run pytest -m server`. Keep these
tests serial so simulated stations can meet control-loop deadlines. Pytest
builds the station on demand. To use an existing binary, set
`HEBBIAN_RUA_SERVER` to its absolute path.

The optional SO-101 policy preview tests exercise the real model through ONNX
Runtime. Set `ORT_DYLIB_PATH` to a compatible runtime and place the private
checkpoint at `.cache/so101-policy-assets/policy.onnx`, then run:

```sh
cargo test --locked -p hebbian-rua-core --test preview -- --include-ignored
```

## Generated contracts and SDKs

Edit the owning Rust types or the [station route table](../station/src/openapi.rs),
then regenerate the schemas and SDKs from the repository root:

```sh
uv run rua-dev contracts
```

Use `--targets python` or `--targets python rust` for a partial run. Before
merging a shared wire change, regenerate all affected SDKs and review hand-written
adapters and callers, including lease requirements and retry behavior.

After committing the generated files, run `uv run rua-dev contracts --check`.
It regenerates files and compares the result with Git HEAD, including staged
changes and untracked generated outputs. The
[contracts generator](../tooling/rua_dev/contracts.py) owns the output paths.

## Dependency refreshes

Refresh only the affected lockfiles. Preserve exact pins, compatibility bounds,
and the three-day release delay in uv and pnpm.

```sh
uv lock --upgrade
uv lock --upgrade --project examples/python
uv lock --upgrade --project examples/mujoco_sim
pnpm update --recursive --include-workspace-root --lockfile-only
cargo update
```

Change exact manifest versions or Git revisions before refreshing their locks.
See [Contributing](../CONTRIBUTING.md#making-a-change) for dependencies we maintain.

### SO-101 dependency

Update the [hardware runtime pin](../server-core/Cargo.toml), refresh `Cargo.lock`,
and run the affected station tests. To update robot models, change the revision
and checksums in [the asset manifest](../robots/so101-assets.toml), run
`uv run rua-dev setup`, and check the affected profiles and geometry.

## SO-101 reachability sweep

With the pinned robot assets installed, run the planner benchmark:

```sh
RUA_REACH_SWEEP_OUTPUT=/tmp/rua-reach.jsonl \
  cargo test -p hebbian-rua-core --lib reachability_sweep -- --ignored
```

Use the same fixtures and seeds when comparing revisions. The
[benchmark source](../server-core/src/planning/tests/reach_sweep.rs) defines the
replay controls and contains the companion `impossible_pose_latency` benchmark.

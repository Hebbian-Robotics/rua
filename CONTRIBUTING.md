# Contributing

Follow [source setup](docs/install-from-source.md#prepare-the-checkout) and
install [source-doc-lint](tooling/source-doc-lint/README.md) for code changes.
Follow the [TypeScript SDK setup](typescript-client/README.md#package-builds)
for TypeScript changes.

Choose checks for the component you are changing:

| Change | Start with |
| --- | --- |
| Documentation | Local link check below |
| Python SDK or tooling | `uv run rua-dev validate --lane python` |
| TypeScript SDK | `pnpm check` and `pnpm --filter hebbian-rua test` |
| TypeScript example applications | `pnpm check` and `pnpm --filter @hebbian/agent test` |
| Rust control or station behavior | A focused Cargo test, such as `cargo test --locked -p hebbian-rua-server --bin hebbian-rua-server so101` after setup |
| Shared wire types | Regenerate from the owning Rust types with `uv run rua-dev contracts` |

Run `uv run rua-dev validate --help` for the lanes in your checkout.
[Development reference](docs/development.md) explains generated-file checks.

## Making a change

Use the [architecture guide](docs/architecture.md) to identify the component
that owns the behavior, then update affected callers and documentation. For
dependencies we maintain, make and validate the generic fix upstream before
updating this repository’s pins and lockfiles. Keep public dependencies generic
and keep private configuration, calibration, credentials and recordings out of
commits.

For an architecture review, inspect the Rust workspace dependency tree on demand:

```sh
cargo tree --locked --workspace --depth workspace --edges normal --all-features --target all
```

For shared SDK changes, check the SDKs included in your checkout and document
intentional differences. See [SDK implementations](docs/architecture.md#client-sdks)
for their locations and adapter boundaries.

Keep changes scoped and inspect the diff for unrelated edits or generated-file
drift. Describe the resulting behavior, how it was checked, and any unverified
integration or hardware behavior. Update the existing topic guide alongside a
change rather than adding another overview of the same workflow.

## Validation

Run the affected language’s formatting, lint and type checks, then the relevant
tests. Start with targeted checks and broaden when the change crosses a boundary;
avoid redundant builds and full test runs.

Run the [comment checker](tooling/source-doc-lint/README.md) for code changes:

```sh
source-doc-lint --config source-doc-lint.toml
```

For Python:

```sh
uv run ruff check --fix
uv run ruff format
uv run ty check
```

If your `pyproject.toml` includes `[tool.importlinter]`, also run
`uv run lint-imports`.

The server and full lanes run workspace Rust checks. For those lanes,
install the [media build prerequisites](docs/cameras.md#rua-configuration) and
the pinned dependency checkers:

```sh
cargo install cargo-machete --version 0.9.2 --locked
cargo install cargo-deny --version 0.20.2 --locked
```

```sh
cargo fmt
cargo machete --with-metadata --skip-target-dir
uv run rua-dev check-architecture
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace --all-features
```

For TypeScript, when included:

```sh
pnpm format
pnpm check
```

`uv run rua-dev --help` lists this checkout’s generation, validation and packaging
commands. Run the appropriate validation lane for a broader check. Check the
assembled distribution when changing package contents; source checks alone do
not establish what users install. Simulation does not validate physical hardware.

For documentation-only changes, verify commands against their CLI/source and
check local links and section anchors:

```sh
lychee --offline --include-fragments --extensions md README.md CONTRIBUTING.md docs examples
```

Check affected component READMEs too. Use the same selection without
`--offline` for external links when needed.

## Testing

Test observable outcomes at the layer that owns the behavior, using the least
expensive boundary that can demonstrate it. Prefer extending existing coverage.
Do not test mocks, third-party internals, implementation steps or constants merely
to repeat their defaults. Use independent expected results.

Use pure logic tests where possible. Integration tests should protect a distinct
process, transport or lifecycle boundary; keep their setup narrow and avoid
repeating owner-level assertions. Keep tests independent. When consolidating
coverage, identify where each meaningful assertion remains. Use the test runner's
skip or ignore mechanism for optional prerequisites. When several guards can
reject an input, use a passing control that differs only in the condition under
test. Report skipped tests and validation limits.

## Code style

- Use descriptive function and variable names and explicit domain types.
- Keep control flow direct. Simplify responsibilities rather than adding helpers
  solely to satisfy complexity limits; keep test inputs and expected results clear.
- Prefer typed variants, exhaustive matching and boundary parsing.
- Share operations across entry points rather than duplicating behavior.
- Preserve and report unknown external values when recovery is possible.
- Keep library capabilities reusable; examples may specialize them.
- Enforce each constraint in the component that owns it.

## Documentation

Keep user guides clear and self-contained, with the steps and examples needed
to complete the task. Developer docs should explain intent and non-obvious
constraints, linking to owning code instead of repeating implementation details.
Keep shared procedures in one place and document current behavior, not discovery
history. Update existing topic guides rather than adding overlapping ones.

Link to the owning dependency’s documentation for its workflows and internals;
keep RUA integration instructions in this repository.

## Robot profiles

`profiles/` ships two runnable SO-101 starting points, `so101` for hardware and
`so101-sim-only-no-real-arm` for the simulator socket. Use incomplete TOML
fragments for settings shared across scenarios. Keep runnable combinations in
the owning examples and keep per-station calibration and ports in local profiles.
The [server-core profile test](server-core/src/profile/tests.rs)
`bundled_profiles_offer_only_hardware_and_simulator_presets` checks the two
bundled entry points.

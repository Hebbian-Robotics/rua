# Contributing

Feedback, issue reports and contributions are welcome. Follow the
[source setup](docs/install-from-source.md) for the development environment.

## Making a change

Use the [architecture guide](docs/architecture.md) to identify the component
that owns the behavior, then update affected callers and documentation. For
dependencies we maintain, make and validate the generic fix upstream before
updating this repository’s pins and lockfiles. Keep public dependencies generic
and keep private configuration, calibration, credentials and recordings out of
commits.

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

Run the [comment checker](tooling/comment-policy/README.md) for code changes:

```sh
uv run python tooling/comment-policy/check.py
```

For Python:

```sh
uv run ruff check --fix
uv run ruff format
uv run ty check
```

If your `pyproject.toml` includes `[tool.importlinter]`, also run
`uv run lint-imports`.

For Rust, install the pinned dependency checkers once:

```sh
cargo install cargo-machete --version 0.9.2 --locked
cargo install cargo-deny --version 0.20.2 --locked
```

```sh
cargo fmt
cargo machete --with-metadata --skip-target-dir
uv run rua-dev check-architecture
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
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

Check affected component READMEs too. Check public-export templates in an
assembled checkout, where relative links resolve. Use the same selection without
`--offline` for external links when needed.

## Testing

Test observable outcomes at the layer that owns the behavior, using the least
expensive boundary that can demonstrate it. Prefer extending existing coverage.
Do not test mocks, third-party internals, implementation steps or constants merely
to repeat their defaults. Use independent expected results.

Use pure logic tests where possible. Integration tests should protect a distinct
process, transport or lifecycle boundary; keep their setup narrow and avoid
repeating owner-level assertions. Keep tests independent. When consolidating
coverage, identify where each meaningful assertion remains. Report skipped tests
and validation limits.

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
keep RUA integration instructions in this repository. Review public-export
overlays and included files when changing shared setup or package guidance.

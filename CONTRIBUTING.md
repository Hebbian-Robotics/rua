# Contributing

This is an early release of part of our robotics stack. Feedback and issue reports
are welcome.

For development, follow [source setup](docs/install-from-source.md). The pinned Rust toolchain includes
Clippy and rustfmt. Python checks run on 3.14; the package supports Python 3.12+.
Python dependencies keep a three-day minimum release age.

For Python changes:

```sh
uv run ruff check --fix
uv run ruff format
uv run ty check
uv run pytest
```

`uv run rua-dev validate --lane python` runs the Python checks and tests together.
Use `uv run rua-dev contracts --check` to verify schemas and the Python SDK, or
omit `--check` to regenerate them. Targets default to SDKs present in the checkout.

For Rust changes:

```sh
cargo fmt
cargo clippy --all-targets --all-features
cargo test --workspace --all-features
```

`uv run rua-dev validate` checks all included components. Native prerequisites
are required for its Rust checks. To verify a built wheel outside the checkout,
use `uv run rua-dev package check /path/to/hebbian_rua.whl`.
Build the bundled release wheel with `uv run rua-dev package rua`. The Publish
workflow builds and tests on manual runs; publishing a GitHub release also
uploads the verified wheel to PyPI and attaches it to the release.

Check guide links with `lychee --offline -v README.md CONTRIBUTING.md docs`;
use the same command without `--offline` for external links. Check export
templates in an assembled checkout, where their relative links resolve. Automatic CI runs only in `Hebbian-Robotics/rua`;
other checkouts use the same commands locally.

The media build needs the prerequisites in [camera setup](docs/cameras.md).
Rust workspace tests also need [FFmpeg](docs/recording.md) on `PATH`.
Keep credentials, recordings and per-arm calibration out of commits.

For camera changes, verify a real USB or phone stream with LiveKit: browser
video, SDK snapshots, simultaneous readers, reconnection and shutdown. Simulator
checks do not cover a physical camera or arm.

Additional component commands appear in `uv run rua-dev --help` when included.
Use targeted checks first. Simulation does not validate physical hardware.

## Code style

- Use descriptive function and variable names, even when they are verbose.
- Prefer explicit types and typed variants; make invalid states hard to represent.
- Use exhaustive matching where practical, and parse external values at boundaries.
- Share internal operations instead of duplicating behavior across entry points.
- Preserve and log unknown external values when the boundary permits recovery.
- Comment on non-obvious decisions rather than restating the code.
- Test observable outcomes and real business logic, not mocks or library internals.
- Use uv for Python. Keep changes scoped and run targeted checks first.
- Keep control and SDK boundaries reusable; examples do not restrict downstream workflows.
- Enforce each limit once, clamp or degrade valid requests, and trip on physical evidence.
  Diagnose hardware from measurements; do not loosen limits to conceal a problem.

Use the workspace's existing Clippy allowances and Python lint settings. The
release uses `panic = "abort"`; default test builds retain unwinding. Configure
sccache in your own Cargo settings rather than requiring it for contributors.

## Documentation boundaries

Keep setup commands, configuration and behavior specific to RUA here. Link to
[SO-101](https://github.com/Hebbian-Robotics/so101),
[LiveKit Cameras](https://github.com/Hebbian-Robotics/livekit-cameras) and
[MCAP Tools](https://github.com/Hebbian-Robotics/mcap-tools) for their workflows,
APIs and internals. Keep a complete RUA quickstart; avoid copying their guides.

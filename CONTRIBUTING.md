# Contributing

This is an early release of part of our robotics stack. Feedback and issue reports
are welcome.

For development, follow [source setup](docs/getting-started.md#from-source), then
run `uv sync --locked` from the checkout root. The pinned Rust toolchain includes
Clippy and rustfmt. Python checks run on 3.14; the package supports Python 3.12+.
Python dependencies keep a three-day minimum release age.

For Python changes:

```sh
uv run ruff check --fix
uv run ruff format
uv run ty check
uv run pytest
```

For Rust changes:

```sh
cargo fmt
cargo clippy --all-targets --all-features
cargo test --workspace --all-features
```

Check documentation links with `lychee --offline -v .`; use `lychee -v .` to
include external links. CI checks Python and Rust on Ubuntu 24.04.

The media build needs the prerequisites in [camera setup](docs/cameras.md).
Rust workspace tests also need [FFmpeg](docs/recording.md) on `PATH`.
Keep credentials, recordings and per-arm calibration out of commits.

For camera changes, verify a real USB or phone stream with LiveKit: browser
video, SDK snapshots, simultaneous readers, reconnection and shutdown. Simulator
checks do not cover a physical camera or arm.

## Code style

- Use descriptive function and variable names, even when they are verbose.
- Prefer explicit types and typed variants; make invalid states hard to represent.
- Use exhaustive matching where practical, and parse external values at boundaries.
- Share internal operations instead of duplicating behavior across entry points.
- Preserve and log unknown external values when the boundary permits recovery.
- Comment on non-obvious decisions rather than restating the code.
- Test observable outcomes and real business logic, not mocks or library internals.
- Use uv for Python. Keep changes scoped and run targeted checks first.

Use the workspace's existing Clippy allowances and Python lint settings. The
release uses `panic = "abort"`; default test builds retain unwinding. Configure
sccache in your own Cargo settings rather than requiring it for contributors.

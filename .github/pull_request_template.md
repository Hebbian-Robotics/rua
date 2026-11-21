## What changed and why

## How you checked it

- [ ] `pnpm check` and `pnpm test:viewer` (viewer changes)
- [ ] `uv run --no-sync ruff check`, `ruff format`, `ty check` and `pytest` (Python changes)
- [ ] `cargo clippy --all-targets --all-features`, `cargo fmt` and `cargo test` (executor changes)
- [ ] Simulation only, or tried on a real arm (say which arm and what you checked)

## Notes for reviewers

Robot-motion or safety-limit changes: describe the clearance and supervision you used.

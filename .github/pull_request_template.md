## What changed and why

## How you checked it

- [ ] `pnpm check` and the relevant TypeScript SDK or agent tests
- [ ] `uv run --no-sync ruff check`, `ruff format`, `ty check` and `pytest` (Python changes)
- [ ] `cargo clippy --all-targets --all-features`, `cargo fmt` and `cargo test` (controller changes)
- [ ] Simulation only, or tried on a real arm (say which arm and what you checked)

## Notes for reviewers

Robot-motion or safety-limit changes: describe the clearance and supervision you used.

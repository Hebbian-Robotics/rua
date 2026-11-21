# Testing RUA

Follow [Contributing](../CONTRIBUTING.md#testing) for test design and review
expectations. This page covers the repository’s test infrastructure.

## Integration tests

Shared Python helpers live in `tests/fixture_*.py`; avoid importing one test
module from another. Validation lanes and test invocation details are in the
[development reference](../docs/development.md#validation-details).

TypeScript integration tests carry both `server` and `typescript` markers.
The full validation lane includes them; the server lane excludes `typescript`
so Rust and Python development does not require Node.js or pnpm.

## Performance

Ordinary tests verify that combined arm motion and gripper opening complete.
The separate wall-clock check retains the profile's motor-cycle deadline. Run
it on an idle host in release mode, with one test thread:

```sh
cargo test --release -p hebbian-robot-use-core --lib control_loop::tests::combined_motion_and_opening_meet_the_motor_cycle_deadline -- --ignored --exact --nocapture --test-threads=1
```

Shared CI scheduling can delay a thread beyond this budget. This measurement
is separate from the deterministic timer tests and does not validate hardware.

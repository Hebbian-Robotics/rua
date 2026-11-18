# Architecture

The station process owns the HTTP API, control leases, compilation artifacts
and recording lifecycle. Its supervised controller owns the motor buses and
coordinated control loop. The media companion owns camera capture and publishing.
Agent loops run in clients; the station does not call a model.

## Components

| Owner | Location | Responsibility |
| --- | --- | --- |
| Shared contracts | `contracts/` and `schemas/` | Typed requests, outcomes and generated SDK contracts |
| Robot Use Compiler | `executor/src/compiler.rs` | Structural analysis and typed robot operations |
| Artifact store | `executor/src/station_api/programs.rs` | Saved compilations, validity, retention and inspection |
| Robot Use Runtime | `executor/src/station_api/runtime.rs` | Live step preparation, coordinated admission and truthful outcomes |
| Station host | `station/` | HTTP composition and process supervision |
| Motor controller | `executor/src/control_loop/` | Feedback, setpoints, protective checks and plant commands |
| Camera companion | `livekit-cameras/` | Capture, snapshots, publishing and recording video |
| Recording library | `recordings/` | Retained recordings and playback access |

## Robot Use Compiler and Runtime

Ordinary action-sequence submissions compile automatically. The Robot Use
Compiler validates the sequence against configured arm capabilities and
translates it into typed operations. Unchanged operations can reuse structural
work. Advanced users can compile and inspect a sequence before acquiring a lease.

The Robot Use Runtime prepares each step from live robot state and admits its
participating commands together. Admission checks the lease, revisions,
interlocks and cancellation. The motor controller executes admitted commands
and applies final protective checks. Compilation and step preparation happen in
the station, outside the motor cycle.

The station retains temporary compilation artifacts for inspection and reuse.
Each execution has its own ID and outcome. See the
[control model](control-model.md) and
[compilation guide](control-model.md#compile-before-running) for execution,
artifact and continuation semantics.

## Client SDKs

This source subset includes the Python SDK in `hebbian_rua/client/`. Its generated
contracts and HTTP methods come from the shared Rust definitions and OpenAPI.
Provider adapters in `hebbian_rua/client/llm/` handle model-specific messages;
the common station client owns HTTP, leases and execution outcomes.

The TypeScript and Rust SDKs live in the development repository and are not
included in this subset. Shared contract changes must be coordinated across those
implementations before release. Keep server code independent of SDKs and
provider adapters. Use the [source setup](install-from-source.md) and
[contributing guide](../CONTRIBUTING.md) for validation commands.

# Architecture

The station process owns the HTTP API, control leases, compilation artifacts
and recording lifecycle. Its supervised controller owns the motor buses and
coordinated control loop. The media companion owns camera capture and publishing.
Agent loops run in clients; the station does not call a model.

## Components

| Owner | Location | Responsibility |
| --- | --- | --- |
| Shared contracts | [Rust contracts](../contracts/src/lib.rs) and [OpenAPI](../schemas/openapi.json) | Typed requests, outcomes and generated SDK contracts |
| Robot Use Compiler | [`server-core/src/compiler.rs`](../server-core/src/compiler.rs) | Structural analysis and typed robot operations |
| Artifact store | [`server-core/src/station_api/programs.rs`](../server-core/src/station_api/programs.rs) | Saved compilations, validity, retention and inspection |
| Robot Use Runtime | [`server-core/src/station_api/runtime.rs`](../server-core/src/station_api/runtime.rs) | Live step preparation, coordinated admission and truthful outcomes |
| Station host | [Station composition](../station/src/serve.rs) | HTTP composition and process supervision |
| Motor controller | [Motor loop](../server-core/src/control_loop/mod.rs) | Feedback, setpoints, protective checks and plant commands |
| Camera companion | [Media CLI](../livekit-cameras/src/main.rs) | Capture, snapshots, publishing and recording video |
| Recording library | [Recording library](../recordings/src/library.rs) | Retained recordings and playback access |

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

The [Python SDK](../hebbian_rua/client/README.md) and [Rust SDK](../sdk/README.md)
use generated routes from the shared Rust definitions and OpenAPI.
Provider adapters in `hebbian_rua/client/llm/` handle model-specific messages;
the common station client owns HTTP, leases and execution outcomes.

The [command-line client](../cli/README.md) uses the Rust SDK. Keep server code
independent of SDKs and provider adapters. Use the
[source setup](install-from-source.md) and [contributing guide](../CONTRIBUTING.md)
for validation commands.

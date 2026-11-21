# Architecture

The robot server consists of a station process (`hebbian-robot-use-server serve`)
and its supervised motor-controller process. The station owns the HTTP API, the viewer's
static files, the lease and the recordings library. The controller owns the arm
buses and their coordinated control loop. Independent guards watch enabled arms.
For cameras, the station starts and supervises `hebbian-robot-use-media serve`;
LiveKit publishing is also started when configured. The MuJoCo simulator is a
separate process you start. Clients use the station's HTTP API and event stream;
camera snapshots come from the media service and the viewer receives live video
through LiveKit. The robot server does not run agent loops: the client owns the
loop. The active robot profile
supplies model paths, hardware mapping and limits. See the [station reference](reference.md)
for commands and operational details. In the station, action sequences pass
through the [Robot Use Compiler and Robot Use Runtime](#robot-use-compiler-and-runtime)
before reaching the controller.

## Embodiments and controller support

Robot integration and learned-policy availability are separate capabilities.

| Embodiment | Current integration | Learned control |
| --- | --- | --- |
| Single SO-101 | Hardware and MuJoCo profiles; IK/joint motion | Experimental SO-101 adapter; weights are not distributed and physical policy control is unvalidated |
| Bimanual SO-101 | Hardware and MuJoCo profiles; coordinated IK/joint trajectories | No coordinated learned policy integration |

IK uses the configured serial chain and joint limits rather than a hardcoded
SO-101 chain. This does not establish hardware support for another embodiment.
Learned models may have different joint layouts, observations, actions, history
and rates. A bimanual or whole-body model may control a coordinated group rather
than one arm. The current single-arm policy interface does not yet implement
that group scope.

## Vocabulary

- **Robot server** includes the station and its motor controller.
- **Motor controller** tracks arm targets and applies final protective checks.
- **Client** reads state or sends commands through the station API.
- **Agent loop** uses a model to choose which robot tools to call.
- **Agent server**: an optional program that runs agent loops in its own process (it calls
  `runAgent` from `@hebbian/agent`; it does not spawn them), holds the model keys, keeps run
  records and exposes a run API (`examples/typescript/viewer/server/`).
- **Viewer**: the browser UI. **CLI**: `hebbian-robot-use`, one command per call.
- **Viewer connections**: the viewer talks to the station for state and manual
  control, to LiveKit for video, and to the optional agent server for the run
  lifecycle (start, stop, list and read).
- The **lease holder** is the client authorized to send motion commands. See the
  [control model](control-model.md) for lease and interlock behavior.
- An **agent run** is one execution of an agent loop. The agent server records
  the run, and the robot server relays its live telemetry.

A task starts an agent run. The run contains model turns and tool calls; its
trace contains spans describing that work. A run can complete, stop, be
interrupted, fail, or end incomplete before the model finishes.

```text
viewer ── state and manual control ──► station ──► motor controller ──► robot
   │
   ├────── video ───────────────────► LiveKit ◄── media service ◄── cameras
   │
   └────── run lifecycle ───────────► agent server
                                         │ runs an agent loop
                                         └── SDK ──► station

Other SDK clients, scripts and the CLI also connect to the station.
```

### Teams and shared robots

Many viewers can watch one robot at once, but one holder controls at a time. Each
person runs their own agent server (and holds their own model keys). A shared network
is assumed trusted: the robot server has no authentication, and a stop is accepted
from anyone.

## Components

| Component | Location | Responsibility |
| --- | --- | --- |
| Station contracts | `contracts/`, generated `schemas/` | Commands, events and schemas consumed by clients and internal transports |
| Robot server core | `server-core/` | Profile resolution, motion control, planning and station API services |
| Station host | `station/` | HTTP composition, controller/media supervision, recording lifecycle and live run telemetry |
| Camera access | `station-access/` | Station-owned LiveKit credentials, scoped token grants and invite policy; see [station access](../station-access/README.md) |
| Media companion | `livekit-cameras/` | The `hebbian-robot-use-media` binary: capture/publish orchestration, snapshots and recording video with station telemetry |
| Recording library | `recordings/` | List and serve saved MCAP recordings with byte ranges; see [recording library](../recordings/README.md) |
| Client SDKs and CLI | `hebbian_rua/client/`, `typescript-client/`, `sdk/`, `cli/` | Typed station access, model-facing robot tools, reconnect/resume and command outcomes |
| Reference applications | `examples/` | Client loops, agent/provider integrations, agent server and browser viewer; see [examples](../examples/README.md) |
| Simulator adapter | `examples/mujoco_sim/` | Profile-specific MuJoCo scenes and station plant/camera protocols |
| SO-101 setup package | [`Hebbian-Robotics/so101`](https://github.com/Hebbian-Robotics/so101) | Independent calibration engine, native setup store, browser UI and canonical model assets; see [package guide](https://github.com/Hebbian-Robotics/so101/tree/main/setup) |
| RUA Python adapters | `packages/rua-tools/src/hebbian_robot_use_tools/` | Asset paths and binary discovery |
| Development tools | `tooling/rua_dev/` | Contract generation, validation, packaging, licenses and public source export |
| Robot configuration/assets | `profiles/`, `robots/` | Limits, embodiment mappings, simulator scenes and model references |

The station resolves robot profiles in
[`server-core/src/profile/`](../server-core/src/profile/mod.rs). The MuJoCo
fixture reads the resolved per-arm JSON, and the viewer takes arm placement
from station state. The standalone SO-101 setup package supplies the native
`ArmSetup` format. The server depends on shared contracts and the hardware
adapter. SDKs access it over HTTP.

Rust's reach assessment takes a pose and velocities; the live loop and preview
supply them and act on its explicit running, settled or timed-out result.
[`Recovery decisions`](../server-core/src/control_loop/recovery.rs) similarly take
observations and elapsed time; the runner performs bus operations. SDK tool
parsers produce typed operations before execution assigns request identities.

The media companion is separate so station/control binaries do not link
libwebrtc. The external
[livekit-cameras](https://github.com/Hebbian-Robotics/livekit-cameras) repository
owns reusable capture, publishing, latest-frame access, room video recording and
browser invites/QR codes. The external
[mcap-tools](https://github.com/Hebbian-Robotics/mcap-tools) repository owns grouped
MCAP writing, standard message encodings and optional video export. RUA retains
its access grants, recording lifecycle, telemetry topics and interruption policy.

## Client SDKs

The station has three SDK implementations:

| SDK | Location |
| --- | --- |
| Python | `hebbian_rua/client/` |
| TypeScript | `typescript-client/` |
| Rust | `sdk/` |

Each provides typed station access and model-facing robot tools. Shared commands,
schemas, outcomes and model-output behavior span all three implementations.
See [model output](model-output.md) for shared formatting contracts and fixtures.

Provider adapters have different owners: Python supplies them under
`hebbian_rua/client/llm/`; the TypeScript reference adapters live in
`examples/typescript/agent/`; Rust applications assemble provider messages.

## Station and controller code boundaries

The internal `hebbian-robot-use-core` library supplies the motion and HTTP
services. The `hebbian-robot-use-server` package in `station/` builds the
server command of the same name. The controller runs as a supervised
subcommand of that server. `rua` launches the server from the Python
distribution, and `hebbian-robot-use` is the client CLI.

| Module | Owns | Boundary |
| --- | --- | --- |
| [Compiler](../server-core/src/compiler.rs) | Robot Use Compiler, including structural validation and operation reuse | Reads action-sequence source and configured arm capabilities. |
| [Artifact store](../server-core/src/station_api/programs.rs) | Compilation artifacts, retention, inspection and persistence | Shares retained artifacts with the runtime. |
| [Runtime](../server-core/src/station_api/runtime.rs) | Robot Use Runtime, including step preparation, admission, continuation and execution progress | Uses live feedback and the shared control link to admit steps. |
| [Station API](../server-core/src/station_api/mod.rs) | HTTP routes, request deduplication/cancellation, station state assembly and resumable client events | The host uses `HttpServer`, viewer/trace path helpers and the run telemetry interface. Implementation modules are private. |
| [Control link](../server-core/src/control_link.rs) | Arm command handoff, planning access, feedback publication and retained per-arm records | `LinkHandle` is the client-facing endpoint; the loop consumes its inbox and publishes feedback without calling HTTP handlers. |
| [Motor loop](../server-core/src/control_loop/mod.rs), [arm state](../server-core/src/arm/mod.rs) | Coordinated motor cycles, command state and final protective checks | Reads and commands a `Plant`; receives admitted commands through the handoff. |
| [Bridge](../server-core/src/controller_bridge.rs), [transport](../server-core/src/controller_transport.rs), [service](../server-core/src/controller_service.rs) | Station/controller request framing and transport workers | Transport I/O runs outside the motor cycle; messages carry the shared command and feedback types. |
| [Lease](../server-core/src/lease.rs) | Lease ownership, admission fencing and shared interlock state | Shared coordination used by the link; HTTP-specific interlock monitoring lives in `station_api/interlocks.rs`. |
| `station/` | Process supervision and composition of robot, media, access and recording services | Mounts application routes on the API and supplies its run telemetry store. |

`ArmSetup` takes ownership of its plant; outside the control loop it exposes only
an immutable plant reference and disabled-startup verification. Clippy restricts raw
servo handles and motor-write methods to the hardware adapter, control loop,
independent guard, simulated preview and explicit maintenance paths.

`station_api` exposes its hosting interface from `mod.rs`; callers must not depend
on its route, retry or event-buffer internals. Changes to HTTP behavior belong
there, while plant I/O and setpoint checks belong in the controller modules.

This is an internal module boundary, not a separate library or a complete
transport-independent control engine. The station's link owns planning and lease-aware admission. The controller uses
the shared handoff and publishes feedback through its link.

## Who controls the arms

A single station-wide lease coordinates manual control and agent loops.
See the [control model](control-model.md) for ownership, power, interruption
and recovery semantics.

## Robot Use Compiler and Runtime

The [compiler](../server-core/src/compiler.rs) validates action-sequence structure
and arm capabilities before any lease is needed. It can reuse structural work
for unchanged operations. The [artifact store](../server-core/src/station_api/programs.rs)
retains recent compilations in memory. Explicit compile and import requests
also save a compilation manifest outside the compiler lock.

The [runtime](../server-core/src/station_api/runtime.rs) prepares each step from
live feedback and admits all participants together. Live lease, revision and
protective checks apply on every execution. Keeping these stages separate lets
a client inspect and reuse a compilation while plans remain grounded in the
arm's current state. Both stages run outside the motor cycle.

See [explicit compilation](control-model.md#compile-before-running) for the
client workflow.

## What the server guarantees

The [station state assembler](../server-core/src/station_api/state.rs) combines
feedback with command records. An active command clears after feedback sampled
following its end. A later completed command can replace an earlier outcome in
`last_command`; use the command record to inspect a particular result.
Read [HTTP and event access](reference.md#http-and-event-access) for request
identity, replay and cursor expiry.

## Control responsibilities and hardware boundaries

One controller owns the final setpoints for each arm. Planning and clients
supply targets while the controller owns live tracking and protective checks.
See [motion path boundaries](controller.md#motion-path-boundaries) for planned,
streamed and learned control and [control invariants](control-invariants.md)
for admission and failure handling.

## Internal robotics boundaries

Keep these components in this repository while their interfaces evolve with the
control loop. A module boundary does not imply a stable external package API.

- **Geometry:** [`robot-geometry`](../robot-geometry/README.md) owns URDF loading,
  serial-chain poses and Jacobians, bounded inverse kinematics and discrete
  collision queries. It uses shared request and outcome types from `contracts`
  and is independent of station configuration and motion admission.
  `server-core/src/ik.rs` adapts the resolved arm configuration to the solver.
  Existing geometry module paths reexport the package.
- **Motion admission and safety checks:** `server-core/src/planning/`, `self_collision.rs`,
  `surface_guard.rs` and control-loop guards decide which motions are allowed.
  Sampling resolution, clearance settings and table recovery rules stay in the
  executor. The shared collision model answers point queries with a
  caller-supplied allowance; the executor samples paths and admits motions.
- **SO-101 hardware:** the independent [Rust runtime crate](https://github.com/Hebbian-Robotics/so101/tree/main/runtime)
  owns servo register I/O, retries/timing, encoder/model conversions, preflight,
  torque enable, feedback, goals and verified recovery. It also owns runtime
  motor config, per-arm wiring/alignment, LeRobot calibration/limits, USB
  serial matching and non-overwriting calibration installation. Cargo enforces
  its dependency direction: it imports no station, planner, policy or client.
  `server-core/src/so101/driver.rs` implements the station's `Plant` interface by
  translating hardware feedback, recovery and communication failures. Moving
  buffers across that adapter adds no per-cycle telemetry copies. The station
  retains motion admission, limits, timing and its independent process guard.
  Hardware facts and motor-check behavior remain with the driver.
- **SO-101 setup and host configuration:** the independent
  [setup distribution](https://github.com/Hebbian-Robotics/so101/tree/main/setup) owns the Python setup
  engine, direct USB metadata discovery, register/gain operations, native
  `ArmSetup` contract, browser UI and model assets. Its source and installed wheel
  work without this stack. `server-core/src/so101/discovery.rs` loads the native arm
  setup and applies RUA control defaults; it also supports inherited profiles; `executor_config.rs` passes host
  gripper force settings to the runtime's calibration/config loader.
  The SO-101 ONNX policy and its integration remain in the executor.
  Model sources live in the SO-101 repository; `robots/so101` contains licensed
  copies from the pinned revision for existing profiles and asset builds. The full viewer imports the package's model rendering helpers; the
  package imports no station UI or client types. Python import contracts enforce
  that direction. See [dependency updates](so101-dependency.md). Physical workflow
  validation still requires the particular arm.
- **Simulation:** `examples/mujoco_sim/mujoco_runtime.py` selects the rendering
  backend, `scene.py` constructs the profile-specific scene, `sim_plant.py`
  maps station targets to MuJoCo actuators using `hebbian_rua/simulator/` for
  socket transport, and `camera_server.py` adapts rendered views to the media
  protocol. Scene setup still depends on this repository's profiles and assets.

Prefer strengthening these boundaries as behavior changes require it. Extract
another repository when a second consumer can exercise the reusable portion,
without carrying station configuration or motion admission rules with it.

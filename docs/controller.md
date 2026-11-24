# Motor controller

The controller runs the motor cycle separately from the station and its clients.
It is a soft-real-time loop on a general-purpose OS. Serial I/O, scheduling,
torque changes and synchronous policy inference can all delay a cycle.
Measure on the deployed host under representative load.

- [Process and module ownership](architecture.md#station-and-companion-processes)
- [Plan admission and failure responses](control-invariants.md)
- [Hardware operation](hardware.md)
- [Simulator adapter contract](simulators.md)

## Diagnostic logs

The station, controller and media process write timestamped diagnostics to
stderr. Set `RUST_LOG` when starting the station to choose levels by module.
For example, `RUST_LOG=warn,hebbian_rua_core::guard=debug` shows guard
startup details alongside warnings and errors. The station's child processes
inherit the filter. Guard reports and the bound HTTP address are emitted
independently of it.

## Motion path boundaries

| Input | Final reference owner |
| --- | --- |
| Planned joint-space sections | [Trajectory execution](../server-core/src/control_loop/trajectory.rs) and [scalar path generator](../server-core/src/corner_path.rs) |
| Single-arm pass-through waypoints | [Local Ruckig waypoint solver](../server-core/src/continuous_path.rs) |
| Changing joint destinations and normal braking | [Reference filter](../server-core/src/reference_filter.rs) |
| Learned pose goals | [Learned reach and stream handling](../server-core/src/control_loop/learned.rs) |

Direct moves follow straight joint-space sections. The scalar trajectory
targets the actual section endpoint and retains commanded velocity and
acceleration between updates. Sending those setpoints through the destination
filter again would change the checked path. [Tracking regressions](../server-core/tests/tracking.rs)
exercise bounds, path geometry and stuck-arm behavior.

A continuous move uses Ruckig to compute section durations and share velocity
and acceleration across section boundaries. The planner checks the generated
joint path for joint limits and collisions, including motion between waypoints.
The controller confirms each crossing from measured feedback within the
configured following lag and before the next section ends. It starts any
waypoint jaw command after confirmation and a successful arm setpoint write.
It waits for the final jaw command before completing the move.

Grouped trajectories share a progress gate and use feedback from the same cycle.
The [group implementation](../server-core/src/control_loop/group.rs) handles member
failure and coordinated stopping. All paths share the [final setpoint guards](../server-core/src/control_loop/guards.rs).
Planning uses a snapshot, so the loop still checks live feedback. A protective
trip can override normal trajectory braking.

Each powered cycle computes its joint and jaw setpoints before writing either.
The runner owns the writable plant. Actuation methods on `Plant` require a
permit that the runner creates and lends to adapters for the duration of a call.
The runner publishes motion completion and checkpoints after the write is
confirmed. A grouped step waits for confirmation from every member. Protective
trips are reported even if the hold write fails.

After enable turns on torque, the controller checks the startup hold over
successive cycles. Other arms continue running during those observations. The
enabling arm accepts motion only after verification succeeds.

## Learned control

The SO-101 ONNX adapter converts pose goals and recent joint feedback into
joint targets. The runner evaluates the policy synchronously when the cycle
requests it, then passes the result through the usual setpoint guards. See
[learned-policy setup](hardware.md#learned-policies) for profile configuration.
The [adapter](../server-core/src/so101/policy.rs) defines the model's inputs,
observation history, action scaling and required metadata. Set
`ORT_DYLIB_PATH` if the station needs an explicit path to ONNX Runtime.

For simulation, use the [MuJoCo policy profile](../examples/mujoco_sim/profiles/policy.toml)
with a compatible model.

### Previewing a policy pose

A preview reports planned IK joints for an IK arm and the rehearsal endpoint
for a policy arm. [Preview](../server-core/src/preview.rs) uses the shared
[reach assessment](../server-core/src/policy/reach.rs) with its own policy history
and a stand-in with no physics. It leaves live commands and policy state untouched.
The stand-in has no load, backlash, table or contact, so the result only estimates
policy behavior without them. Apply the [physical arm guide](hardware.md) when
operating hardware.

## The cycle trace

The [trace writer](../server-core/src/trace.rs) defines the JSONL fields, queue
capacity and drop accounting. Use `uv run hebbian-rua traces list` to find traces
and `uv run hebbian-rua traces pull` to download gzip-compressed files.

Each row contains feedback from the start of the cycle and the command computed
from that feedback. Measure command-to-motion delay from one row's `writeTime`
to the first later reading that moves. Compare `seconds` across consecutive
cycle numbers to measure real intervals. The controller caps the integration
interval in `cycleSeconds` while the deadline guard uses uncapped time.
Trace drops create gaps. Phase timings include OS scheduling delays.

To investigate a slow cycle, separate request admission, feedback read,
reference computation and writes using the timing fields. The [motor loop](../server-core/src/control_loop/mod.rs) defines phase order and
where admission yields. A batch budget cannot preempt an individual request or serial
operation. Use the [SO-101 dependency guide](development.md#so-101-dependency) to locate the
pinned runtime that owns servo retries and environmental checks. Retain both the
original and confirmation readings when investigating temperature or voltage
trips.

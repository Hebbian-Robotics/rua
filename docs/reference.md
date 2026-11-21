# Station reference

Use [getting started](getting-started.md) for a physical arm,
[simulation](simulation.md) for the development fixture, and
[write your own loop](write-your-own-loop.md) for SDK examples. This page covers
station operation; the complete wire contract is [`schemas/openapi.json`](../schemas/openapi.json).

## Station server

From a source checkout, build and start the station for a registered SO-101:

```sh
uv run cargo run -p hebbian-robot-use-server -- serve --profile auto
```

With an installed wheel, [create an explicit hardware profile](hardware.md)
and run `uv run rua serve --profile "$HOME/.config/rua/so101.toml"`. The Python
launcher runs the installed station binary. In the full checkout,
`hebbian-robot-use` is a separate CLI that sends requests to a running station.
See [source installation and updates](install-from-source.md).

The profile selects the arms, model, hardware mapping and limits. For physical
SO-101 hardware, use a calibrated local profile or `--profile auto` with a
[registered arm](so101-manual-control.md#automatically-select-a-registered-so-101).
The bundled hardware profiles contain placeholder serial paths and calibration
IDs; they are not ready-to-run hardware configurations.

| Service | Default address | Purpose |
| --- | --- | --- |
| Station | `127.0.0.1:8792` | State, commands, leases, events, viewer, recording and camera access grants |
| Media companion | `127.0.0.1:8781` | Camera discovery and JPEG snapshots |
| Optional example agent server | `127.0.0.1:8793` | Hosts the TypeScript reference loop; start it separately |

`--http-address` and `--media-address` select the first two addresses. Other
serve options include `--arm`, `--camera name=device`, `--manual-control`,
`--room`, `--enable-recording`, `--recordings-directory`, `--work-directory` and
`--trace-directory`. Run `serve --help` for the current options and defaults.
`--camera name=none` removes a profile camera. `--no-physics-stand-in` replaces the arms with a
stand-in whose joints ease toward their commands; it has no contacts, table, gravity,
loads or camera rendering, so use it to exercise the station, not to judge motion.

The station supervises the controller and media children. An exited media child
is restarted after three seconds; media failures leave control and state access
available. `GET /viewer/api/media-status` reports child status and camera errors.
Each running station needs its own work directory.
Controller failure leaves the station available for inspection and explicit
recovery; it does not automatically re-enable the arm.

Station code, hardware profiles and environment changes require a restart.
Support and disable the arm first. Graceful controller shutdown can attempt a
return to its recorded rest pose before releasing torque; do not use process
termination as a substitute for an explicit supported disable. If the controller
fails while powered, an independent guard may hold and later release the arm.
See [fault recovery](so101-manual-control.md#recovering-from-a-fault).
Viewer assets are read from disk and need only a rebuild and browser refresh.

### Allowed self-contact

Profiles can omit specific benign link contacts from self-collision checking:

```toml
allowed_collision_pairs = [["first_link", "second_link"]]
```

Replace these placeholders with collision-bearing link names from the selected
URDF. Each allowance covers all geometry on both links, in either pair order,
throughout their full range of motion. Unknown names and same-link pairs fail
when the collision model loads. Use this for contacts that are acceptable across
that range; other link pairs retain their collision checks. Neighboring joints
and geometry attached to the same joint are already excluded.

Allowed pairs are removed before runtime queries, which can reduce control-cycle
work. Increasing `self_collision_penetration_tolerance_meters` still requires
computing overlap depth. Neither setting changes the table surface guard.
Apply profile changes with the supported, disabled arm and station restart
described above.

### HTTP and event access

Use the [generated OpenAPI contract](../schemas/openapi.json) for routes,
parameters and payloads. The [route definitions](../station/src/openapi.rs)
own the generated descriptions. For the compile, inspect, import and run
workflow, see [explicit compilation](control-model.md#compile-before-running).

Submissions carry a UUID v4 request ID and station instance ID, plus a lease token
when required. Retrying the same body with the same ID follows the original
result. Changing the body or replaying it against another station instance is
rejected. Do not invent a new request identity to recover a lost reply.

Resume `/events` with the last event ID in `Last-Event-ID`. HTTP 409 means the
cursor is unavailable; open a fresh stream. The event journal retains 4,096
events. The station retains up to 10,000 command outcomes and 10,000 lease
acquisition outcomes. Evicted request IDs remain fenced for the process lifetime:
a retry returns HTTP 410 and does not execute again. New requests are temporarily
refused if all retained entries are in flight.

### SDK recovery behavior

| Situation | Python | TypeScript | Rust |
| --- | --- | --- | --- |
| Transport failure while submitting a command | Retries the original request identity | Retries the original request identity | Returns a connection error; no automatic command retry |
| Event stream disconnects | Reconnects with the last cursor | Reconnects with the last cursor | Reconnects with the last cursor |
| Lease connection is lost | Invalidates local ownership | Invalidates local ownership | Invalidates local ownership |
| Caller cancels a pending leased command | Attempts submission cancellation and a leased stop | Attempts submission cancellation and a leased stop | Dropping the future attempts cancellation and a leased stop in the background |

Telemetry reconnection does not restore ownership or prove command completion.
Inspect the outcome and reacquire control before new motion. Cancellation is
best effort if communications are unavailable; it does not confirm a physical stop.

## Control, tools and results

The [control model](control-model.md) explains leases and power. Motion requires
a lease and an enabled arm. An interlock refuses motion until recovery; an
`expected_revision` from the last observation fences commands after an
interruption. SDKs renew leases and expose commands through the lease handle.

The library’s default model tools are `action_sequence`, `preview_pose`,
`observe` and `stop`. Both reference agents specialize sequences to one SO-101
with Cartesian movement and gripper actions. The library preset also allows
joint targets. The Python example adds `finish`; the TypeScript example adds
`finish`, `update_plan` and `update_notes`. These are application tools, not robot
library capabilities. See [tool selection](write-your-own-loop.md#with-a-model-four-pieces).

`observe` defaults to current state and camera images. Use
`{"source": "last_sequence"}` to request retained images from the latest action
sequence; history is not included in automatic observations. See
[model observations](model-output.md#images-during-action-sequences).

An action sequence is ordered steps. Within a step, movement and gripper targets
start together; the next step waits for completion. A named-arm specialization
can coordinate several arms in the same step. All participating targets are
planned before admission. A stop, failed/limited action or interruption ends the
sequence rather than proceeding blindly. Direct SDK operations remain available
independently of the model catalog.

`stop` interrupts arm and gripper motion and holds with torque enabled. It also
ends an affected sequence. It does not finish the agent run or disable the arm.
The ordinary SDK stop is accepted from anyone; lease-bound stop helpers fence
cleanup against a new holder after handover. `disable` removes torque.

Model arguments/results use millimetres, degrees and degrees per second; direct
station calls use metres and radians, except jaw yaw, which is expressed in
degrees. Gripper opening is normalized from 0 closed to 1 open, not a measured
finger separation. See [model output](model-output.md) for formatting and raw data.

A completed motion can have a limited planned target. Inspect outcomes, achieved
poses, residuals and limit details before deciding what to do next. A gripper
that stops short may hold an object or be blocked; this does not prove a grasp.
Trips hold the arm; hard motor faults can release torque. Bus disconnection is
a separate recovery path and does not deliberately switch torque off.

## Pose preview — no motor commands

The model-facing `preview_pose` accepts a list of Cartesian candidates:

```json
{"poses":[{"pose":{"position_mm":[200,0,150]}}]}
```

Each pose may include an orientation. Results remain in input order without
transport IDs. Direct SDK batch previews retain caller IDs and optional labels.
A batch checks independent candidates, not a sequence or a shared state snapshot.

Quaternion inputs use `[x, y, z, w]`. Norm errors up to `0.00011` are
accepted and normalized for rotation calculations so four-decimal preview
orientations can be reused in moves. Larger errors and nonfinite values are refused.

A preview uses IK, calibrated joint limits and configured geometry checks.
Without an orientation, the solver chooses one. In a move, omitted orientation
keeps the current one; pass the same explicit orientation to preview and move
when comparing a specific request. A constrained projection reports its feasible
local result and the shortfall; it is not a globally closest-pose guarantee.
For an explicit orientation, IK moves and previews retry at tighter pose
tolerances if the initial solution meets the pose tolerance but hits a collision
guard. The refined target is used only if its joint-space path clears the guards.

Motion follows a checked joint-space path, which need not be a straight Cartesian
line. Preview is optional: use it to answer a specific feasibility question.
Neither a preview nor model-derived tool state measures physical clearance or
camera calibration. See [commands and outcomes](write-your-own-loop.md#commands-and-outcomes).

## Cameras and the viewer

See [camera setup](cameras.md) for USB/phone feeds and credentials. The station
starts the media companion and, with LiveKit configured, the USB publisher.
SDK clients read JPEGs from the media service; viewer video uses LiveKit.
Without LiveKit, local USB and simulator snapshots can still serve SDK clients.
The media binary must be built separately for source runs.

The full viewer in this checkout is served at `/viewer/` after `pnpm build`.
`HEBBIAN_VIEWER_DIRECTORY` selects another static asset directory. Installed
public wheels exclude the full viewer. See [viewer development](../examples/typescript/viewer/README.md)
and [manual control](so101-manual-control.md).

For a single SO-101, the viewer loads the arm's configured model and meshes from
`/model/<arm>/robot.urdf`. For multi-arm views, the viewer uses the first arm's
model and built-in base positions. The soft-gripper-camera model includes the
fingers, bracket and conservative camera envelope. Verify cables and workspace
clearance on the physical arm. Camera optical extrinsics remain uncalibrated.

## SO-101 in MuJoCo

The source-only [MuJoCo fixture](simulation.md) runs as a separate process,
connected through each arm’s plant socket. It supplies overview/wrist views,
actuator feedback and single/multi-arm stepping. It contains no task objects or
grasping scenario. Physics settings belong to that fixture, not station profiles.
Public users can [connect their own simulator](simulators.md).

Bimanual profiles place two arms in a shared world using `base_poses`. Check
those transforms against the real mounting before hardware pose commands.
Both arms run in one coordinated controller loop with separate serial buses.
For group moves and sequence steps, admission checks clearance between arms
that have planned trajectories, configured base poses and collision geometry.
Verify clearance from stationary arms and nearby objects yourself. See
[coordinated arm clearance](control-model.md#coordinated-arm-clearance).

## Guard process and stop behaviour

An independent guard watches the powered hardware controller’s heartbeat. After
controller loss it can take over the serial connection, supervise a hold, and
release torque on a violation or hold timeout. This still depends on the host,
power and serial link. The configured timeout lives in the selected profile.

| Event | Response |
| --- | --- |
| `stop` | Brake arm motion and hold; stop the jaw at its current position |
| Tracking, effort or deadline trip | Stop and hold; inspect the outcome before retrying |
| Hard motor fault | Release torque; an unsupported arm can fall |
| Serial bus failure | Last servo goals may continue; reconnect and verify a hold or require operator recovery |
| Controller failure | Station remains available; leases are revoked and explicit supported recovery is required |
| Station shutdown | Controller shuts down; an active guard can take over if release fails |

Follow the [recovery guide](so101-manual-control.md#recovering-from-a-fault)
for the actual operator steps. A software stop is not a physical power cutoff.

## Viewer operation

The viewer shows measured-joint FK, commanded destinations, telemetry and run
records. A ghost is a model prediction, not independently measured tracking.
For controls, use the [manual viewer guide](so101-manual-control.md#controls).
For recording and standalone playback, use [recording](recording.md); the recorded
signals and gaps are described in [media integration](media-integration.md).

### Self-clipping protection

The station checks [contact-tolerant collision geometry](so101-self-clipping.md)
in previews, planned paths and runtime references. These sampled checks cover
configured self-geometry, not unknown objects or another separate arm.

### Table surface guard

The surface height and the model allowance are separate values in the arm’s base
frame. For an SO-101 standing on its workspace tabletop, the nominal surface is
Z = 0: the bundled base model has horizontal contact faces at that height. A
raised mount or a measured offset needs a profile override. The modeled floor is
`surface_height_meters - leeway_meters`; height 0 and leeway 0.01 mean a guard
limit of −0.01 m, not a table at that height. It checks
the lowest moving model geometry, including the selected gripper/camera assembly,
rather than the tool-frame height alone.

```toml
[surface_guard]
enabled = true
surface_height_meters = 0.0
leeway_meters = 0.01
```

Planning and runtime checks refuse further descent below that floor while
allowing recovery upward from below it. Paths are sampled rather than checked
continuously. The configured plane does not detect the actual table or objects;
verify the physical model and mounting before motion.

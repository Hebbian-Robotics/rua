# Station reference

Use [getting started](getting-started.md) for a physical arm,
[simulation](simulation.md) for the development fixture, and
[write your own loop](write-your-own-loop.md) for SDK examples. This page covers
station operation; the complete wire contract is [`schemas/openapi.json`](../schemas/openapi.json).

## Station server

From a source checkout, build and start the station for a registered SO-101:

```sh
uv run cargo run -p hebbian-robot-use-server -- serve
```

With the published `0.1.2` wheel, use the
[released hardware profile format](https://github.com/Hebbian-Robotics/rua/blob/v0.1.2/docs/hardware.md)
and run `uv run rua serve --profile "$HOME/.config/rua/so101.toml"`. The Python
launcher runs the installed station binary.
`hebbian-robot-use` is a separate CLI that sends requests to a running station.
See [source installation and updates](install-from-source.md).

The profile selects the arms, model, hardware mapping and limits. `serve`
defaults to `auto`, which selects a
[registered SO-101](so101-calibration.md).
Pass `--profile` to select a custom hardware profile or a simulator profile.
The bundled hardware profiles contain placeholder serial paths and calibration
IDs; they are not ready-to-run hardware configurations.

| Service | Default address | Purpose |
| --- | --- | --- |
| Station | `127.0.0.1:8792` | State, commands, leases, events, viewer, recording, camera discovery, snapshots and access grants |
| Media companion | `127.0.0.1:8781` | Direct camera discovery and JPEG snapshots |

`--http-address` and `--media-address` select the first two addresses. Other
serve options include `--arm`, `--camera name=device`, `--manual-control`,
`--room`, `--enable-recording`, `--recordings-directory`, `--work-directory` and
`--trace-directory`. Run `serve --help` for the current options and defaults.
Use `--disable-camera-forwarding` for [direct-only camera access](cameras.md#local-snapshots).
`--camera name=none` removes a profile camera. `--no-physics-stand-in` replaces the arms with a
stand-in whose joints ease toward their commands; it has no contacts, table, gravity,
loads or camera rendering, so use it to exercise the station, not to judge motion.

The station supervises the controller and media children. An exited media child
is restarted with backoff up to three seconds. Media failures leave control and
state access available. `GET /viewer/api/media-status` reports child status and camera errors.
Each running station needs its own work directory.
Controller failure leaves the station available for inspection and explicit
recovery; it does not automatically re-enable the arm.

Station code, hardware profiles and environment changes require a restart.
Support and disable the arm first. Graceful controller shutdown can attempt a
return to its recorded rest pose before releasing torque; do not use process
termination as a substitute for an explicit supported disable. If the controller
fails while powered, an independent guard may hold and later release the arm.
See [control invariants](control-invariants.md) for failure handling.

### HTTP and event access

Use the [generated OpenAPI contract](../schemas/openapi.json) for routes,
parameters and payloads. The [route definitions](../station/src/openapi.rs)
own the generated descriptions. For the compile, inspect, import and run
workflow, see [explicit compilation](control-model.md#compile-before-running).

Submissions carry a UUID v4 request ID and station instance ID, plus a lease token
when required. Retrying the same body with the same ID follows the original
result. Changing the body or replaying it against another station instance is
rejected. Do not invent a new request identity to recover a lost reply.
Cancelling before admission fences the request ID. After admission, cancellation
stops only the commands associated with that request that are still in progress.
For a stream, cancel the request that started it. The station retains that
request while the stream runs, even when older update replies are evicted.

Resume `/events` with the last event ID in `Last-Event-ID`. HTTP 409 means the
cursor is unavailable; open a fresh stream. The event journal retains 4,096
events. The station retains up to 10,000 command outcomes and 10,000 lease
acquisition outcomes. Evicted request IDs remain fenced for the process lifetime:
a retry returns HTTP 410 and does not execute again. New requests are temporarily
refused if every retained entry is pending or owns active motion.

### SDK recovery behavior

| Situation | Python | TypeScript | Rust |
| --- | --- | --- | --- |
| Transport failure while submitting a command | Retries the original request identity | Retries the original request identity | Returns a connection error; no automatic command retry |
| Event stream disconnects or receives no bytes for 30 seconds | Reconnects with the last cursor | Reconnects with the last cursor | Reconnects with the last cursor |
| Lease connection is lost | Invalidates local ownership | Invalidates local ownership | Invalidates local ownership |
| Caller cancels a pending leased command | Cancels the submitted request | Cancels the submitted request | Dropping the future attempts request cancellation in the background |
| Caller cancels after an acceptance-only call returns | Keep waiting for completion if task cancellation is needed | An abort signal stays attached until the accepted command finishes | Keep polling the submission future if drop-to-cancel is needed |

The SDKs reconnect immediately after losing a usable event stream. If replay
is unavailable, they clear the cursor and request a fresh snapshot. Failed
attempts use jittered backoff capped at one second. Applying a station snapshot
or event starts a fresh retry budget.
Telemetry reconnection does not restore ownership or prove command completion.
Inspect the outcome and reacquire control before new motion. Cancellation is
best effort if communications are unavailable; it does not confirm a physical stop.

## Control, tools and results

See the [control model](control-model.md) for leases, command outcomes, action
sequences and explicit compilation. The [SDK workflow](write-your-own-loop.md)
covers direct commands and model tool selection. See
[model output](model-output.md) for units, camera images and response formats.

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

## Cameras

See [camera setup](cameras.md) for USB/phone feeds and credentials. The station
starts the media companion and, with LiveKit configured, the USB publisher.
SDK clients request JPEGs through the station by default.
Without LiveKit, local USB and simulator snapshots can still serve SDK clients.
The media binary must be built separately for source runs.

See the [hardware guide](hardware.md#supported-hardware) for supported setups,
[MuJoCo quickstart](simulation.md) for the example fixture and
[simulator interface](simulators.md) for other engines.

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

See [control invariants](control-invariants.md) for the conditions that stop motion. A software stop is not a physical power cutoff.

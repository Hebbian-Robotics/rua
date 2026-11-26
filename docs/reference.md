# Station reference

Follow [getting started](getting-started.md) to set up a physical arm.
Use [simulation](simulation.md) to test without hardware. The
[OpenAPI contract](../schemas/openapi.json) describes the station API.

## Station server

The station serves state, commands, events, camera access and recordings.
`hebbian-rua` is a [separate client CLI](../cli/README.md). The
[selected profile](install-from-source.md#start-the-station) supplies the arms,
model, hardware mapping and limits. Use a separate work directory for each
running station. See [serve options](../station/src/commands/mod.rs) or run
`uv run cargo run -p hebbian-rua-server -- serve --help` for flags
and defaults.

The station supervises the controller and the media companion. A media
restart interrupts snapshots and live publishing while control and state
remain available. `GET /media/status` reports service and camera errors. The
station resolves LiveKit and recording requirements at startup. See
[configuration](cameras.md#rua-configuration) for local-only mode and FFmpeg.
After a controller failure, use explicit recovery before resuming control.

Support and disable the arm before restarting the station to apply code,
profile or environment changes. A graceful shutdown can attempt to return to
the recorded rest pose before releasing torque. See
[failure handling](control-invariants.md#failure-responses).

### Check a running station

```sh
rua status --camera wrist
```

The check looks for advancing arm feedback, which works with torque off, and
requests a fresh JPEG from the named camera. Leave out `--camera` to skip the
snapshot check. For a station on another address, pass `--server URL`.
If the station serves snapshots only from its media companion, pass
`--media-server URL`.

The LiveKit check briefly joins the room without publishing or subscribing to
tracks. It reports whether video is published, the room is waiting for cameras,
or the join failed. With LiveKit off, local station and camera checks still
run. A failed check returns a nonzero exit status.

### HTTP and event access

Use the [OpenAPI contract](../schemas/openapi.json) for routes and payloads.
The [station route table](../station/src/openapi.rs) owns those descriptions.

- To retry a submission after a lost reply, reuse the operation ID and the
  original request body. A different body with that ID conflicts. An expired
  outcome returns HTTP 410 on retry. Inspect or cancel a running request by
  operation ID. See [request tracking](../server-core/src/station_api/submissions.rs).
- Send stream goal updates with increasing sequence numbers for the same
  operation ID. The stream keeps the speed set when it started.
- Resume the event stream with the last event ID. HTTP 409 means the cursor
  is no longer available. Open a fresh stream to get a snapshot. See
  [event replay](../server-core/src/station_api/events.rs).
- Add `X-RUA-Run-Cursor` to subscribe to run records on the same HTTP stream.
  Use `latest` to begin with the current run snapshot or
  `<station-instance>:<sequence>` to resume after a delivered run record. The
  response echoes the current instance in this header. Run events are sent
  as SSE events named `run`. An expired run cursor or a station restart
  produces `run_resync` and a fresh run snapshot while state replay uses
  its own cursor.
- Request [observations](../station/src/openapi.rs) to read arm state and camera
  snapshots together. Required cameras must return an image. The returned arm
  revisions let you reject motion based on a stale observation. A lease holder
  can add a `wait` with the selected arm, lease token and a deadline of up to
  five seconds. The station waits for arm recovery and fresh required
  camera frames and reports a timeout, an arm that became disabled or faulted,
  or HTTP 403 if the lease was lost.

### SDK recovery behavior

The SDKs resume station state and run telemetry with independent cursors and
refresh the affected snapshot when its history expires. See [control leases](control-model.md#control-leases)
for what to verify before resuming motion after an interruption.

The SDKs retry transient submission failures for commands and action
sequences for up to 30 seconds. Each retry sends the same request and operation
ID to the original station instance. The station returns the original admission
and rejects reuse of that ID for different work, including across operation
types. An action sequence's `request_id` becomes its execution ID.

The station retains up to 10,000 submission receipts across commands and action
sequences. It starts evicting finished sequence records at 100 and keeps
running work available. Evicted submission IDs remain reserved until the
station restarts. A retry sent to a different station instance returns a conflict.

When a caller waits for a command to finish, the SDKs use
`POST /operations/{id}/completion` on the station that accepted it. This
endpoint returns the terminal command records and the event sequence of a state
that reflects the result. If the controller lost feedback, the state reports
that loss. The SDKs wait for that event sequence to reach their state cache
while connected. A station advertises this capability via `GET /instance`.
The SDK rejects a completion-waiting command before submission if the station
needs an upgrade. Admission-only submissions still return as soon as admitted.
Cancel a command or action sequence through `POST /operations/{id}/cancel`
using its original station instance and lease. This also fences a submission
that has yet to reach the station.

The SDKs wait for action sequences by execution ID through
`GET /action_sequence/{id}/completion`. The station returns the terminal
record, `null` if the sequence is still running after twenty seconds, or
HTTP 404 if the execution is unknown or has been evicted. A caller can
stop waiting without cancelling the execution. Use the updated SDKs with
a station that exposes this route.

## Control, tools and results

See the [control model](control-model.md) for leases, command outcomes, action
sequences and [explicit compilation](control-model.md#compile-before-running).
[Write your own loop](write-your-own-loop.md) covers direct commands and tool
selection. [Model output](model-output.md) describes the data shown to models.

## Pose preview — no motor commands

Preview checks one or more independent Cartesian targets against the current
arm configuration. Results stay in input order. See the [tool catalog](../schemas/robot-tools.json)
for model-facing inputs and the [planner](../server-core/src/planning/mod.rs) for
IK and path checks.

With no orientation supplied, preview lets IK choose one. A move preserves the
current orientation. To compare a specific pose, supply the same explicit
orientation in both calls. Quaternion components are ordered `[x, y, z, w]`.

A constrained preview reports a feasible result and how far it falls short.
Motion follows a checked joint-space path. Check physical clearance separately.
See [commands and outcomes](write-your-own-loop.md#commands-and-outcomes).

## Cameras

[Camera setup](cameras.md) covers USB, phone and LiveKit feeds. Start the station
with `--local-only-no-remote-cameras` to serve local USB and simulator snapshots
to SDK clients without LiveKit. See
[simulator integration](simulators.md) for other engines.

## Guard process and stop behaviour

An independent guard watches the powered controller. Following controller
loss it can supervise a hold and release torque if the hold becomes unsafe
or times out. It depends on the host, power and serial link.

A software stop brakes and holds the arm. Disabling releases torque, and an
unsupported arm can fall. A physical power cutoff operates separately from the
controller. See [failure responses](control-invariants.md#failure-responses)
for protective stops and recovery.

### Station-bound sequence executions

Sequence status, completion and cancellation accept either the execution ID or
`{station_instance_id}~{execution_id}` in the existing ID path segment. The
station checks the instance before looking up or cancelling the execution. A
different instance returns HTTP 409, including after a restart at the same URL.
SDK execution handles retain the instance from the accepted submission and
the connection used for it. Create a new connection when selecting a different
station. A TypeScript URL callback is evaluated once at connection creation.
Deploy the updated station before using execution handles or the agent loops
that use them. Earlier stations only understand bare execution IDs.

# hebbian-robot-use-cli

The `hebbian-robot-use` command line for a station, built on the `hebbian-rua` Rust SDK.
Coding agents can chain these commands in the shell. Each invocation opens a
connection and runs one command. Commands print JSON to stdout, with plain-text
output for the phone invite URL.

```sh
cargo install --locked --path cli            # installs `hebbian-robot-use`
hebbian-robot-use --help            # units, argument shapes and an example per command
hebbian-robot-use state
hebbian-robot-use enable arm
hebbian-robot-use move-joints arm --joints 0.3,-0.4,0.6,0.1,0.0 --speed 0.5
hebbian-robot-use move-pose arm --position 0.2,0,0.1 --approach down --jaw-yaw-degrees 0
hebbian-robot-use preview-pose arm --position 0.2,0,0.1
```

These examples use `arm`, the name assigned by native SO-101 setup. Run
`hebbian-robot-use state` to discover the names served by a custom profile.

`--server` (default `http://127.0.0.1:8792`) is the station URL, including for
camera requests. Set `--media-server` or `HEBBIAN_MEDIA_SERVER` to use the
companion directly, typically at `http://127.0.0.1:8781`. Set
`HEBBIAN_STATION_URL` to change the default station URL. The older `HEBBIAN_SERVER`
name is a fallback when `HEBBIAN_STATION_URL` is unset.
Input units: radians, metres, rad/s, gripper 0 closed to 1 open, jaw yaw in degrees.
Output is the concise model view shared with the SDKs (see
[model output](../docs/model-output.md)): millimetres (`position_mm`), degrees (`joints_degrees`),
rounded, nulls omitted, and history and diagnostics left out. `state` also keeps the robot
`description` and the lease holder. `--raw` prints the station's full SI records.

## Commands

`state`; `enable|disable|stop|clear-fault|rehome <arm>`; `move-joints`; `move-pose`;
`move-arms <file|->` (JSON `MoveArmsCommand`); `gripper <arm> <opening>`; `preview-pose`;
`preview-poses <arm> <file|->` (a JSON array of poses, one answer each, in order);
`approve-recovery <arm>`; `--expected-revision ARM=REV` refuses a command as stale once the
arm's revision has moved on; `--take-lease` takes the lease from its holder (operator only);
`action-sequence run <file|->` / `cancel <id>` / `get <id>` (JSON `ActionSequenceRequest`);
`lease status|revoke`; `record start|stop|status`; `phone-invite <label>` (prints the
invite URL, or the full response with `--raw`); `camera list` (name and availability) / `camera snapshot <name> --out <path>` (an unavailable
camera fails with `camera_unavailable`, naming the cameras known and the station's `last_error`);
`traces list` / `traces pull [names...] --output <dir>` (files are written exactly as
served, gzip-compressed).

For live video and phone camera invites, follow the
[LiveKit camera setup](../docs/cameras.md). The hosted page plays the live
stream. The CLI lists cameras, captures JPEGs, and creates phone invites.

## Exit codes and errors

| Code | Meaning |
| --- | --- |
| 0 | completed |
| 1 | refused or not completed: a refusal, or a motion that ended stopped, faulted or limited |
| 2 | usage (bad arguments or input JSON) |
| 3 | connection failure or timeout |
| 4 | lease held elsewhere (`lease_held_elsewhere`): another client holds the lease; the holder is in the error |

Failures print `{"error": {"kind", "detail", ...}}` on stderr, with `reason`, `holder`,
`outcome` or `record` when known. Motion commands wait for their outcome.

## Lease

Motion commands (`rehome`, `move-joints`, `move-pose`, `move-arms`, `gripper`, `action-sequence run`)
take a renewable lease for their invocation and return it on exit. Lease renewal
ends if the client loses its connection; an unrenewed lease expires and stops motion.
`lease revoke` explicitly revokes the current holder without acquiring control.
Power (`enable`, `disable`, `clear-fault`) is separate from
the lease: a motion command on a disabled arm is refused as `not_enabled` and says to send
`enable`. While another holder (an agent, the viewer's manual controls) holds the lease, commands
are refused with exit 4; `stop` is always accepted. Motion is refused while an arm has an interlock (see `state`
and [the control model](../docs/control-model.md)).

```sh
cargo test -p hebbian-robot-use-cli   # the real binary against an in-process controller on the simulated SO-101
```

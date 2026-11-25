# hebbian-rua-cli

The `hebbian-rua` command line for a station, built on the `hebbian-rua` Rust SDK.
Coding agents can chain these commands in the shell. Each invocation opens a
connection and runs one command. Commands print JSON to stdout, with plain-text
output for the phone invite URL.

The `hebbian-rua` Python package provides the command alongside `rua`. Run it
from the agent project that serves the station, so the CLI and station come from
the same locked version. The motion flags below describe the CLI in this
checkout. For a published wheel, see `--help` or the README at its release tag.

```sh
uv add hebbian-rua
uv run hebbian-rua --help     # units, argument shapes and an example per command
uv run hebbian-rua state
uv run hebbian-rua enable arm
uv run hebbian-rua move-joints arm --joints-degrees 17,-23,34,6,0 --speed-degrees-per-second 30
uv run hebbian-rua move-pose arm --position-mm 200,0,100 --approach down --jaw-yaw-degrees 0
uv run hebbian-rua preview-pose arm --position-mm 200,0,100
```

Without a project, pin the station’s version: `uvx hebbian-rua==<station version> state`.
From a source checkout, `cargo run -p hebbian-rua-cli -- state` builds and runs
the current source, and `cargo install --locked --path cli` installs it. The
examples below omit the `uv run` prefix.

These examples use `arm`, the name assigned by native SO-101 setup. Run
`hebbian-rua state` to discover the names served by a custom profile.

`--server` (default `http://127.0.0.1:8792`) is the station URL, including for
camera requests. Set `--media-server` or `HEBBIAN_MEDIA_SERVER` to use the
companion directly, typically at `http://127.0.0.1:8781`. Set
`HEBBIAN_STATION_URL` to change the default station URL. The older `HEBBIAN_SERVER`
name is a fallback when `HEBBIAN_STATION_URL` is unset.
Use `--position-mm` for world positions and `--joints-degrees` for joint
angles. Speed, acceleration, and jerk flags spell out their degree or radian
units. For native station input, choose `--position-m` and `--joints-radians`.
The former unitless flags (`--position`, `--joints`, `--speed`, `--acceleration`,
and `--jerk`) must be migrated before using this CLI.

The CLI emits the concise model view with millimetres and degrees. Use
`--raw` for full station output in native units. Gripper opening runs from
0 (closed) to 1 (open). Jaw yaw uses degrees in CLI flags and model JSON,
and radians in native station JSON.

For `move-arms`, `preview-poses`, and `action-sequence run`, use the SDK model
fields `position_mm`, `joints_degrees`, and
`max_joint_speed_degrees_per_second`. The optional caps are
`max_joint_acceleration_degrees_per_second_squared` and
`max_joint_jerk_degrees_per_second_cubed`. For native station JSON, pass
`--input-station` to that command. See
[Model output](../docs/model-output.md) for more on the shared model format.

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

For a [timed single-arm sequence](../docs/control-model.md#optional-move-timing),
`at_seconds` is in seconds from the start of the step in both JSON formats.

## Set up live cameras

Set `LIVEKIT_URL`, `LIVEKIT_API_KEY`, and `LIVEKIT_API_SECRET` in the station's
environment or `.env`. On the station host, run `rua camera-devices` and repeat
`--camera NAME=DEVICE` for each camera when starting the station. This command
uses device indices `2` and `4`.

```sh
rua serve --camera wrist=2 --camera overhead=4
```

On the station host, open another terminal to list the cameras and get the
room's viewer token.

```sh
hebbian-rua camera list
curl --fail http://127.0.0.1:8792/media/access-token
```

Paste the response's `server_url` and `token` into **Room viewer** on the
[hosted LiveKit Cameras page](https://hebbian-robotics.github.io/livekit-cameras/).
See the [camera guide](../docs/cameras.md) for device selectors, source setup,
and phone invites. This setup is also shown in `hebbian-rua camera --help`.

[Options when LiveKit Cloud is unavailable](../docs/cameras.md#when-livekit-cloud-is-unavailable)

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
take a renewable lease for their invocation and return it on exit.
`lease revoke` explicitly revokes the current holder. Commands refused because
another client holds control exit with code 4. Use `state` to see interlocks
and `stop` to request a stop without claiming control. See
[control leases and command outcomes](../docs/control-model.md#control-leases).

```sh
cargo test -p hebbian-rua-cli   # the real binary against an in-process controller on the simulated SO-101
```

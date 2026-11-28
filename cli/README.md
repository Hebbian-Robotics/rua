# hebbian-rua-cli

The `hebbian-rua` command line for a station, built on the `hebbian-rua` Rust SDK.
Coding agents can chain these commands in the shell. Each invocation opens a
connection and runs one command. Commands print JSON to stdout, with plain-text
output for the phone invite URL.

## Agent skill

Install the `rua-cli` skill for your coding agent with:

```sh
npx skills add Hebbian-Robotics/rua --skill rua-cli
```

Choose the agent and installation scope when prompted. Add `--global` to install
for your user instead of the current project. To refresh an installed skill,
run `npx skills update rua-cli` (add `--global` for a global installation).
The skill provides operating guidance; install the `hebbian-rua` CLI separately
as described below. Its guidance follows the repository's current version, so
check the installed CLI's `--help` if your station uses an older release.

## Install and run the CLI

The published `hebbian-rua` wheel provides the command alongside `rua`. Run it
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
From a source checkout, `cargo run --locked -p hebbian-rua-cli -- state` builds and runs
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

`observation [--arm NAME] [--camera NAME] [--optional-camera NAME] --output DIR`
captures selected arms and saves JPEGs, with per-frame timestamps and missing optional
cameras. The station checks selected arm revisions across capture; frames are not necessarily
simultaneous. Required cameras fail the observation if unavailable.
`operation <id>` reads retained operation status. Cancellation requires the original
lease; use `stop` or `action-sequence cancel` where appropriate.

`compilation compile <file|->` compiles a sequence without moving and prints its native-unit
artifact; `compilation inspect <id>` retrieves it, and `compilation import <file>` recompiles
a saved artifact for this station. Save an artifact using shell redirection. `compilation run
<id>` acquires a motion lease and waits for the result, applying `--expected-revision`.
Compile input uses model units by default or native units with `--input-station`. Artifacts
are always printed in native units so they can be imported unchanged.

`state`; `enable|disable|stop|clear-fault|rehome <arm>`; `move-joints`; `move-pose`;
`move-arms <file|->` (JSON `MoveArmsCommand`); `gripper <arm> <opening>`; `preview-pose`;
`preview-poses <arm> <file|->` (a JSON array of poses, one answer each, in order);
`approve-recovery <arm>`; `--expected-revision ARM=REV` refuses a command as stale once the
arm's revision has moved on; `--take-lease` takes the lease from its holder (operator only);
`action-sequence run <file|->` (model-unit JSON by default, native station
`ActionSequenceRequest` with `--input-station`) / `cancel <id>` / `get <id>`;
`lease status|revoke`; `record start|stop|status`; `phone-invite <label>` (prints the
invite URL, or the full response with `--raw`); `camera list` (name and availability) / `camera snapshot <name> --out <path>` (an unavailable
camera fails with `camera_unavailable`, naming the cameras known and the station's `last_error`);
`traces list` / `traces pull [names...] --output <dir>` (files are written exactly as
served, gzip-compressed).

## Timed action sequences

Save this model-unit example as `steps.json` and run
`hebbian-rua action-sequence run steps.json`. Replace `arm` with the station's
arm name if needed.

```json
{
  "steps": [{
    "arms": {
      "arm": {
        "waypoints": [{
          "at": {"joints_degrees": [5, -30, 30, 0, 0]},
          "at_seconds": 1
        }],
        "goal": {"joints_degrees": [10, -30, 30, 0, 0]},
        "at_seconds": 3
      }
    }
  }]
}
```

Both CLI input formats use the `arms` map and measure `at_seconds` from the
start of the step. See [optional move timing](../docs/control-model.md#optional-move-timing)
for how the station applies those times.

To sample cameras during a sequence, repeat `--camera` on the run command:

```sh
hebbian-rua action-sequence run steps.json --camera wrist --camera overhead
hebbian-rua action-sequence images <execution-id> --output sequence-images
```

When cameras are selected, the run prints `execution_id` alongside its result. The images command saves numbered JPEGs and
prints their camera names, capture times, elapsed times and sequence boundaries, along with
retention, missed sample and truncation information. Capture is best effort and does not
delay motion. It returns `null` while capture is pending or if history is unavailable. A
completed history expires after five minutes or earlier if evicted. Compiled sequence runs
do not currently request camera capture.

## Set up live cameras

Follow the [camera guide](../docs/cameras.md) to configure LiveKit, select
devices, list cameras, and watch their feeds. It also covers phone invites and
[options when LiveKit Cloud is unavailable](../docs/cameras.md#when-livekit-cloud-is-unavailable).
Run `hebbian-rua camera --help` for CLI commands.

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

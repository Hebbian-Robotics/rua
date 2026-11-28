---
name: rua-cli
description: Operate and inspect a Hebbian robot-use station with the hebbian-rua CLI, including arm motion, cameras, recordings and diagnostic traces. Use for terminal-based robot tasks.
---

# RUA CLI

Use `hebbian-rua` to talk to an existing station. This client is separate
from `rua serve`, which starts the station. Both come with the published
`hebbian-rua` Python wheel. In a project that installs the wheel, call it as
`uv run hebbian-rua`; without a project, use `uvx hebbian-rua==<station version>`
so the CLI matches the station. From a RUA source checkout, use
`cargo run --locked -p hebbian-rua-cli --`.

Choose commands according to the task. `state` reports arm names, joint order,
measured joints, faults, interlocks, and the lease holder without changing
torque. It also includes the configured robot description. Each invocation
returns its own result. Camera snapshots are available as separate commands. Each command’s `--help` shows argument shapes and JSON examples.

Global options: `--server URL` (default `http://127.0.0.1:8792`, or `HEBBIAN_STATION_URL`),
`--media-server URL` overrides the camera service (also settable with
`HEBBIAN_MEDIA_SERVER`). Camera calls use the station URL by default. A direct
media companion typically listens on `http://127.0.0.1:8781`.
Check the station's actual port if a connection fails. `--expected-revision ARM=REV` refuses a
command as stale if the arm's revision moved on since you read `state`; `--take-lease` takes the
lease from its holder and needs the user's authorization. Output is the concise
model view (millimetres as `*_mm`, degrees as `*_degrees`, nulls, checkpoints, trace metadata and
render geometry omitted; sequences show progress and the last step; `state` keeps the robot
`description` and lease holder); `--raw` gives the full station records in native units, including everything left out. Each invocation connects,
executes one command and exits. Successful commands print one JSON value, except
`phone-invite` prints a plain-text URL unless `--raw` is set. Failures use stderr
and a nonzero exit code. Motion waits for its outcome.

## Commands

- Inspect: `state`, `lease status`, `camera list`, `traces list`.
- Capture: `observation --camera NAME --output DIR` checks selected arm revisions during capture and saves JPEGs with individual timestamps. Repeat `--arm`, `--camera` or `--optional-camera`; frames are not necessarily simultaneous.
- Operation status: `operation ID` reads a retained command outcome. Exact cancellation requires the original lease; use `stop` or `action-sequence cancel` when appropriate.
- Compiled sequences: `compilation compile steps.json > artifact.json`, `compilation inspect ID`, `compilation import artifact.json`, and `compilation run ID`. Compilation and import do not move; run acquires a lease and waits. Artifacts use native units and can be saved unchanged.
- Power and recovery: `enable`, `disable`, `stop`, `clear-fault`, `approve-recovery`, `rehome` take an arm name. Approve recovery only after inspecting and supporting the arm.
- Move: `move-joints`, `move-pose`, `gripper`; use `preview-pose` (one pose) or `preview-poses <arm> <file|->` (many, answered in order) to inspect feasibility without moving.
- Coordinated moves: `move-arms <file|->`; ordered steps: `action-sequence run <file|->`, `get <id>`, `cancel <id>`. Read their help for JSON contracts; `-` reads stdin.
- Images: `camera snapshot <name> --out <path>` saves a JPEG to inspect; a `camera_unavailable` error lists the cameras the media service knows and the station's `last_error` for that camera (why it failed to open), and `camera list` shows each one's availability; `phone-invite <label>` creates a phone-stream invite.
- Recording: `record start|status|stop`. It continues until stopped, independently of the motion lease.
- Diagnostics: `traces pull [names...] --output <directory>` saves gzip-compressed traces.

## Units and outcomes

Pose targets are in the station world frame. In the standard SO-101 layout,
+X is forward, +Y is left when facing +X, and +Z is up. An arm's configured
base pose can differ from the world origin and orientation. Image directions
depend on the camera pose.

Use `--position-mm` and `--joints-degrees` to match the concise output.
Joint arrays follow the state's `joint_names` order. The CLI also accepts
`--position-m` and `--joints-radians`. Joint speed, acceleration, and jerk
flags spell out their units. Motion JSON uses the SDK model fields
`position_mm`, `joints_degrees`, and `max_joint_speed_degrees_per_second`.
JSON acceleration and jerk caps use
`max_joint_acceleration_degrees_per_second_squared` and
`max_joint_jerk_degrees_per_second_cubed`. Use `--input-station` for native station
JSON. Gripper opening is
0 closed to 1 open. Quaternions use x,y,z,w; jaw yaw uses degrees. Without an
orientation, `move-pose` preserves the current orientation, whereas
`preview-pose` lets the solver choose one. Supply the same explicit orientation
when comparing a preview with a move.

A pose preview checks feasibility without moving the arm. It cannot establish
clearance from objects in the workspace. A limited move can end at an adjusted
target. When present, `pose_limited.advice` describes the shortfall and a reachable
alternative. A `stopped_short` move settled outside tolerance. Its completion
detail describes the remaining error and may leave the cause unknown.

On a gripper close, `completion.gripper.kind` tells you how the jaw stopped.
`stopped_short` indicates contact or a blockage, and `reached` indicates the
requested opening. Neither confirms a grasp by itself.

Motion acquires and releases its own lease per invocation; power is separate.
The station refuses motion on a disabled arm. `enable` applies torque and
holds the current pose. `rehome` returns to the pose saved at enable time.
`stop` holds position; `disable` removes torque, so support the arm first.

Exit codes: 1 refused or incomplete, 2 usage, 3 connection error or timeout,
4 lease held elsewhere. After a timeout, the command’s outcome may be unknown.
Inspect the error and current state before retrying a failed command, especially
before resending motion. Inspect the cause before clearing a fault.
`lease revoke` is an operator override and requires user authorization.
The station’s interlocks and motion limits apply to CLI commands.

---
name: rua-cli
description: Operate and inspect a Hebbian robot-use station with the hebbian-rua CLI, including arm motion, cameras, recordings and diagnostic traces. Use for terminal-based robot tasks.
---

# RUA CLI

Use `hebbian-rua` to talk to an existing station. This client is separate
from `rua serve`, which starts the station. Both come with the `hebbian-rua`
Python package. In the project that runs the station, call it as
`uv run hebbian-rua`; without a project, use `uvx hebbian-rua==<station version>`
so the CLI matches the station. From a RUA source checkout, use
`cargo run -p hebbian-rua-cli --`.

Start with `hebbian-rua --help` and `hebbian-rua state`. Use
`<command> --help` for argument shapes and JSON examples. Discover arm names,
joint order, faults, interlocks and the lease holder from state.

Global options: `--server URL` (default `http://127.0.0.1:8792`, or `HEBBIAN_STATION_URL`),
`--media-server URL` (default `http://127.0.0.1:8781`, or `HEBBIAN_MEDIA_SERVER`) and `--raw`.
Check the station's actual port if a connection fails. `--expected-revision ARM=REV` refuses a
command as stale if the arm's revision moved on since you read `state`; `--take-lease` takes the
lease from its holder and needs the user's authorization. Output is the concise
model view (millimetres as `*_mm`, degrees as `*_degrees`, nulls, checkpoints, trace metadata and
render geometry omitted; sequences show progress and the last step; `state` keeps the robot
`description` and lease holder); `--raw` gives the full SI records, including everything left out. Each invocation connects,
executes one command and exits. Successful commands print one JSON value;
failures use stderr and a nonzero exit code. Motion waits for its outcome.

## Commands

- Inspect: `state`, `lease status`, `camera list`, `traces list`.
- Power and recovery: `enable`, `disable`, `stop`, `clear-fault`, `approve-recovery`, `rehome` take an arm name. Approve recovery only after inspecting and supporting the arm.
- Move: `move-joints`, `move-pose`, `gripper`; use `preview-pose` (one pose) or `preview-poses <arm> <file|->` (many, answered in order) to inspect feasibility without moving.
- Coordinated moves: `move-arms <file|->`; ordered steps: `action-sequence run <file|->`, `get <id>`, `cancel <id>`. Read their help for JSON contracts; `-` reads stdin.
- Images: `camera snapshot <name> --out <path>` saves a JPEG to inspect; a `camera_unavailable` error lists the cameras the media service knows and the station's `last_error` for that camera (why it failed to open), and `camera list` shows each one's availability; `phone-invite <label>` creates a phone-stream invite.
- Recording: `record start|status|stop`. It continues until stopped, independently of the motion lease.
- Diagnostics: `traces pull [names...] --output <directory>` saves gzip-compressed traces.

## Operating rules

Inputs: joint angles are radians in state's `joint_names` order. Positions are metres
in the robot's world frame (x forward, z up); speed is rad/s. Gripper opening is
0 closed to 1 open. Quaternions use x,y,z,w; jaw yaw uses degrees. Without an
orientation, `move-pose` preserves the current orientation, whereas
`preview-pose` lets the solver choose one. Supply the same explicit orientation
when comparing a preview with a move.

A grasp is judged from `completion.gripper`: `{"kind": "stopped_short", ...}` on a close means
the jaw stopped on something (or was blocked). `{"kind": "reached", ...}` means it arrived.
A move's `pose_limited.advice` gives a reachable approach.
Place with the target visible: a held block hides it from the wrist camera.

Motion acquires and releases its own lease per invocation; power is separate.
Enable an arm before authorized motion. Rehome returns to its enable-time pose.
Use `gripper` for a separate jaw command.
`stop` holds position; `disable` removes torque, so support the arm first.

On failure, inspect the error and current state before retrying. Exit codes:
1 refused/incomplete, 2 usage, 3 connection/timeout, 4 lease held elsewhere.
After a timeout, inspect state before resending a motion. Respect another
holder; use `lease revoke` only for a user-authorized takeover. Inspect the cause
before clearing faults; preserve interlocks and safety limits.

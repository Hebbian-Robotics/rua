# Control model

The station manages robot state and accepts commands through its API. A client
acquires a control lease before sending motion commands. Other clients can
continue reading state and camera snapshots.

See [control invariants and failure responses](control-invariants.md) for the
Rust admission guarantees and the controller's handling of invalid inputs.

In Python, use `station.holding_lease(...)` and send motion through the returned
lease. Power, stop and state access are available through `station.arm(...)`.
Commands return outcomes; check them before proceeding to another action.

## Control leases

A control lease expires if the holding client stops renewing it. The station
stops the arms and releases the expired lease after they confirm the stop.
If the stop cannot be confirmed, control stays fenced for explicit recovery.
Changing the holder of an active lease requires an explicit handoff.

After losing a lease, inspect the command outcome and reacquire control before
sending motion. If the connection drops before the station confirms
cancellation, check the command and arm state when the connection returns.

The station enforces configured joint limits and protective stops. These checks
require per-arm calibration and do not establish physical workspace clearance.
See [SO-101 calibration](so101-calibration.md) before operating a physical arm, and the
[Python example](../examples/python/hello_robot.py) for a small control program.

## Command outcomes

An admission refusal leaves that request unapplied. For an admitted command,
the record carries the outcome and the state achieved where available. Check
the outcome and limit details before deciding what to do next.

Command records carry `accepted_unix_nano` and terminal phases carry
`finished_unix_nano`. Arm state carries `sampled_unix_nano` when feedback is
available. These fields use decimal strings of nanoseconds since the Unix epoch.
Compare their integer values when ordering events.

The record's `command.kind` identifies the target or control action.
Its `phase.kind` distinguishes `pending`, `not_started`, `finished`, and
`outcome_unknown`. Only a finished command can carry end measurements.
For a command issued before a controller disconnect, check the phase before
using its result to plan another action. A combined arm-and-gripper step keeps
one record per resource, so each has its own outcome.

| Outcome | Meaning |
| --- | --- |
| `completed` | The planned motion finished. A limited plan can finish at an adjusted target. |
| `stopped_short` | The arm did not stay within the target tolerance for the required dwell before the settle deadline. Inspect the achieved pose and remaining errors. |
| `stopped` or `superseded` | A stop ended the command, or a new move replaced it while settling. |
| `tripped` or `timed_out` | A protective check or deadline ended the motion. Inspect state before retrying. |
| `faulted`, `interrupted` or `failed` | A fault, lost feedback or execution failure ended the command. Inspect the record and recover before continuing. |

A gripper that stops short may have contacted something. Use camera and state
feedback to assess the grasp. `stop` brakes motion and holds with torque
enabled. `disable` removes torque. A stop ends an affected action sequence
while the application still owns its agent loop.

Planned IK motion allows time for the trajectory and tracking speed reductions
before settling starts. If a trajectory section exceeds its deadline, the
controller brakes to a hold and reports `timed_out`. The bundled SO-101 profile
allows two seconds for settling once the reference stops. When that deadline
expires before the dwell completes, the arm holds at the measured pose and
reports `stopped_short`.

## Action sequences

An action sequence contains ordered steps with actions for one or more arms.
The runtime prepares the whole step from current feedback before submitting it
for coordinated admission. It waits for outcomes before starting the next step.
A stop, failed or limited action, or interruption can end execution early.

A move has a required `goal` and optional `waypoints`. The arm passes through
the waypoints, in order, and settles at the goal. A step-level `gripper_opening`
begins with the move. An opening attached to a waypoint begins as the arm passes
that configuration. The controller checks measured joint error at the crossing
and records the checkpoint. Physical jaw travel takes time.

For example, this single-arm model tool call assumes the arm starts at smaller
pan angles than the waypoint. Model joint angles are in degrees.

```json
{
  "steps": [{
    "waypoints": [{
      "at": {"joints_degrees": [10, -30, 30, 0, 0]},
      "gripper_opening": 0.8
    }],
    "goal": {"joints_degrees": [20, -30, 30, 0, 0]}
  }]
}
```

The station resolves Cartesian poses to joints and checks the generated
joint-space trajectory before execution. Continuous moves currently support
one IK-controlled arm per step. If the solver needs to stop at a waypoint,
use two steps so the arm can settle there. A direct move needs only `goal`.

Use separate goal steps for a sequence of stops. The station prepares each
step from current feedback, so an earlier step can complete before a later
step is refused. Station move records report the admitted `destination_joints`
and, when available, `destination_position` and `destination_quaternion`.
The model view reports `destination_joints_degrees` and
`destination_position_mm`. A policy reach records its requested `goal_pose`.
Model-facing `goal_status` indicates whether the move reached its goal or was
limited or interrupted.

Execution depends on the lease and current interlocks. If a disconnected client
stops renewing the lease, expiry stops the arms. Lease handoff, revocation and
interlocks can also stop execution. An expected arm revision fences a request
based on observations from before an interruption.

### Optional move timing

Set `at_seconds` alongside `goal` to request a minimum duration for a move
by one IK-controlled arm. For a continuous move, also set `at_seconds` beside
each waypoint's `at`. These are times from the start of the same step. They
must be finite and non-negative and increase strictly from waypoint to goal.
Omit all times for the default motion.

This example uses the single-arm model tool. For a JSON file you can pass to
`hebbian-rua action-sequence run`, see the [CLI example](../cli/README.md#timed-action-sequences).

```json
{
  "steps": [{
    "waypoints": [{
      "at": {"joints_degrees": [10, -30, 30, 0, 0]},
      "at_seconds": 1.2
    }],
    "goal": {"joints_degrees": [20, -30, 30, 0, 0]},
    "at_seconds": 2.5
  }]
}
```

This requests at least 1.2 seconds for the first section and at least 1.3
seconds for the second. Each section takes as long as its motion limits
require. A late section shifts later arrivals by the same delay. The controller
advances the reference in control cycles and the physical arm can lag behind.
The arm settles at the goal before the next step starts with its own clock.

Use moving waypoints. A waypoint that requires a stop or reversal needs a
separate step to settle there. A positive duration on a direct move requires
joint travel. Timing is available for planned moves in single-arm steps.

### Per-move trajectory caps

Single-arm IK moves accept optional joint speed, acceleration, and jerk caps.
In an action sequence, put these fields on the arm's move alongside `goal`
and `waypoints`.

| Cap | Model tools and default CLI JSON | Station JSON (`--input-station`) |
| --- | --- | --- |
| Speed | `max_joint_speed_degrees_per_second` | `max_joint_speed` (rad/s) |
| Acceleration | `max_joint_acceleration_degrees_per_second_squared` | `max_joint_acceleration` (rad/s²) |
| Jerk | `max_joint_jerk_degrees_per_second_cubed` | `max_joint_jerk` (rad/s³) |

Omitted caps use the arm profile. The station clamps a speed
request to the profile ceiling and refuses acceleration or jerk above the
profile ceiling. Pose previews and completed move records report
`effective_motion_limits`. The acceleration and jerk overrides apply to planned
single-arm moves.

Use `--speed-degrees-per-second`, `--acceleration-degrees-per-second-squared`,
and `--jerk-degrees-per-second-cubed` with the CLI's `move-joints`, `move-pose`,
or `preview-pose` commands. For radians, use `--speed-radians-per-second`,
`--acceleration-radians-per-second-squared`, and
`--jerk-radians-per-second-cubed`. In the viewer, open **Settings →
Advanced motion limits**. The browser saves these caps for planned IK moves
and the agent runs it starts. Manual dragging in Follow mode also uses the
saved speed cap, or the arm profile default when the field is blank.

## Coordinated arm clearance

Before admitting a step with two or more arms, the station checks the
synchronized joint paths of its participating arms. Each needs a configured
base pose, collision geometry and jaw feedback if it has a modeled gripper.
Use planned trajectories for multi-arm steps and move each jaw in a separate
step from the arm trajectories and other jaws. The check allows up to 3 mm of
modeled overlap. Verify clearance from arms outside the step and objects in the
workspace.

## Compile before running

Normal action-sequence submissions compile automatically. To inspect a
sequence before acquiring control, call `station.compile_action_sequence(source)`.
Compilation checks structure and configured capabilities for up to 1,024
ordered steps. The runtime prepares each step from measured state.

```python
from hebbian_rua.client.contract.station_api import (
    ActionSequenceGripper,
    ActionSequenceRequest,
    ActionSequenceStep,
    ProgramCompiled,
)

source = ActionSequenceRequest(
    steps=[ActionSequenceStep(arms={"arm": ActionSequenceGripper(gripper_opening=0.8)})]
)
outcome = await station.compile_action_sequence(source)
if isinstance(outcome, ProgramCompiled):
    print(outcome.artifact.compiled)
    async with station.holding_lease("inspected sequence") as lease:
        execution_id = await lease.start_compiled_sequence(
            outcome.artifact.compilation_id
        )
        result = await station.wait_for_action_sequence(execution_id)
        print(result.phase)
else:
    print(outcome.diagnostics)
```

Enable the arm with `station.arm("arm").enable()` and check its outcome
before running. Compilation can succeed
while an arm is disabled; execution still requires enablement, a current lease
and live admission checks. `inspect_compilation(compilation_id)` retrieves a retained
artifact; an unknown or expired artifact returns HTTP 404. The station retains
up to 128 compilations in memory, preserving those used by active executions.
Compilation IDs are scoped to the running station.

A completed or stopped execution includes its `compilation_id`, requested steps
and actual command records. Starting the compiled program again creates a new
execution beginning at its first step. Inspecting an artifact never moves
hardware.

A stopped result carries `phase.failure` with a source step and a typed `stage`:
`preparation`, `admission`, `admission_unknown`, `execution`, `cancellation`,
`authority` or `runtime`. Preparation failures also identify the arm; preparation
and known admission refusals preserve the station rejection reason.
`admission_unknown` means the controller may have acted without returning its
receipts. Continuation treats that step as attempted and will not repeat it.
Structural compilation rejection remains separate in compilation diagnostics.

The inspection object is also the portable compilation export. Save its JSON
beside a recording or in your own artifact store, then load it through the typed
contract:

```python
from pathlib import Path
from hebbian_rua.client.contract.station_api import CompilationArtifact

artifact_path = Path("compilation.json")
artifact_path.write_text(outcome.artifact.model_dump_json(indent=2))
saved = CompilationArtifact.model_validate_json(artifact_path.read_text())
imported = await station.import_compilation(saved)
```

Run this after the successful-compilation branch above. Import accepts schema
version 3 and always recompiles the saved source against this station.
Saved operations,
configuration identities and compiler versions grant no authority. The imported
result has a new compilation ID and still requires explicit execution.

To continue a finished execution, compile the revised full action sequence and pass an
explicit `ProgramContinuation(prior_execution_id=execution_id, next_step=N)` to
`lease.start_compiled_sequence(..., continuation=...)`. `next_step` is zero-based;
the preceding source prefix must match the prior execution. The runtime rejects
starts that repeat an attempted step, including a partially executed coordinated
step. Inspect those partial outcomes and express any deliberate recovery as new
work. A prior execution must be finished or canceled, and at least one step must
remain. Continued execution records contain only their newly attempted steps and
link the prior execution and starting index.

Explicit compile and import requests save a `compilation.json` inspection file.
Executions of these compilations save records and per-step preparation and
timing snapshots under the
[temporary workspace](../server-core/src/station_api/artifact_workspace.rs).
Preparation snapshots identify the planned source operations. Check the
execution record for admission and completion. Export the inspection JSON or
copy the workspace beside your recording for longer retention.

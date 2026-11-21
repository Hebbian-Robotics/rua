# Control model

The station manages robot state and accepts commands through its API. A client
acquires a control lease before sending motion commands. Other clients can
continue reading state and camera snapshots.

See [control invariants and failure responses](control-invariants.md) for the
Rust admission guarantees and the controller's handling of invalid inputs.

In Python, use `station.holding_lease(...)` and send motion through the returned
lease. Power, stop and state access are available through `station.arm(...)`.
Commands return outcomes; check them before proceeding to another action.

A manual control lease expires if its viewer stops renewing it. The station
stops every arm and releases the expired lease after all arms confirm they have
stopped. You can then enable a disabled arm and take manual control again.
If the stop cannot be confirmed, control stays fenced for explicit recovery.
Changing the holder of an active lease requires an explicit handoff.

The station enforces configured joint limits and protective stops. These checks
require per-arm calibration and do not establish physical workspace clearance.
See [SO-101 calibration](so101-calibration.md) before operating a physical arm, and the
[Python example](../examples/python/hello_robot.py) for a small control program.

## Coordinated arm clearance

Before admitting a step with two or more arms, the station checks the
synchronized joint paths of its participating arms. Each needs a configured
base pose, collision geometry and jaw feedback if it has a modeled gripper.
Use planned trajectories for multi-arm steps and move each jaw in a separate
step from the arm trajectories and other jaws. The check allows up to 3 mm of
modeled overlap. Verify clearance from arms outside the step and objects in the
workspace.

## Compile before running

Normal action-sequence tools compile automatically. An action sequence contains
at most 1,024 ordered steps; actions within each step retain their coordinated
semantics. To inspect a sequence before acquiring control, call `station.compile_action_sequence(source)`. Compilation
checks structural validity and configured capabilities; future steps still need
preparation from measured state during execution.

```python
from hebbian_rua.client.contract.station_api import (
    ActionSequenceArmAction,
    ActionSequenceRequest,
    ActionSequenceStep,
    ProgramCompiled,
)

source = ActionSequenceRequest(
    steps=[
        ActionSequenceStep(arms={"arm": ActionSequenceArmAction(gripper_opening=0.8)})
    ]
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
version 1 and always recompiles the saved source against this station. Saved
operations, configuration identities and compiler versions grant no authority.
The imported result has a new compilation ID and still requires explicit execution.

To continue a finished execution, compile the revised full action sequence and pass an
explicit `ProgramContinuation(prior_execution_id=execution_id, next_step=N)` to
`lease.start_compiled_sequence(..., continuation=...)`. `next_step` is zero-based;
the preceding source prefix must match the prior execution. The runtime rejects
starts that repeat an attempted step, including a partially executed coordinated
step. Inspect those partial outcomes and express any deliberate recovery as new
work. A prior execution must be finished or canceled, and at least one step must
remain. Continued execution records contain only their newly attempted steps and
link the prior execution and starting index.

Explicit compile and import requests save a manifest. Executions of these
compilations save records and per-step preparation and timing snapshots under the
[temporary workspace](../server-core/src/station_api/artifact_workspace.rs).
Preparation snapshots identify the planned source operations. Check the
execution record for admission and completion. Export the inspection JSON or
copy the workspace beside your recording for longer retention.

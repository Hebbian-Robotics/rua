# Control model

The station manages robot state and accepts commands through its API. A client
acquires a control lease before sending motion commands. Other clients can
continue reading state and camera snapshots.

See [control invariants and failure responses](control-invariants.md) for the
Rust admission guarantees and the executor's handling of invalid inputs.

In Python, use `station.holding_lease(...)` and send motion through the returned
lease. Power, stop and state access are available through `station.arm(...)`.
Commands return outcomes; check them before proceeding to another action.

A manual control lease expires if its viewer stops renewing it. The station
stops every arm and releases the expired lease after all arms confirm they have
stopped. You can then enable a disabled arm and take manual control again.
If the stop cannot be confirmed, control stays fenced for explicit recovery.
An active lease still requires an explicit handoff to another controller.

The station enforces configured joint limits and protective stops. These checks
require per-arm calibration and do not establish physical workspace clearance.
See [hardware setup](hardware.md) before operating a physical arm, and the
[Python example](../examples/python/hello_robot.py) for a small control program.

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
        ActionSequenceStep(
            arms={"follower": ActionSequenceArmAction(gripper_opening=0.8)}
        )
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

Enable the arm with `station.arm("follower").enable()` and check its outcome
before running. Compilation can succeed
while an arm is disabled; execution still requires enablement, a current lease
and live admission checks. `inspect_compilation(compilation_id)` retrieves a retained
artifact; an unknown or expired artifact returns HTTP 404. The station retains
up to 128 compilations, preserving those used by active executions. Temporary
files may disappear after external cleanup or reboot.

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

On the station host, compilation JSON, prepared-step snapshots and execution
snapshots live in `rua-compiler-{station_instance_id}/{compilation_id}` under the
platform temporary directory. Each execution has its own `executions/{id}`
directory containing `record.json`, `step-{index}.prepared.json` and
`step-{index}.timing.json` files. Timing snapshots separate state-dependent
preparation from controller admission; compilation manifests report structural
compilation time.
Station files use compact JSON; inspection returns the same structured data,
and callers can format exported JSON as needed. Preparation snapshots identify
source operations;
they are evidence of preparation, not proof of admission or completion. Files are
written outside the motor cycle. At startup, a background job removes inactive
station workspaces older than 24 hours. A live session's directory lock
protects its files.
Export the inspection JSON or copy the workspace directory beside your recording
before relying on it for longer retention.

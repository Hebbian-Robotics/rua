# TypeScript client

Use the TypeScript client to control a running station from a browser or Node.js.
To run an agent first, start with the [reference agent](../examples/typescript/agent/README.md).
It already handles observations, tool execution and default output formatting.

Use this guide when building your own client or changing its tool set.

## Use the workspace package

The package is not published yet; use it from this repository's pnpm workspace.
Workspace commands enable the `development` export condition in tsx and Vite;
imports resolve directly to TypeScript source, so
there is no client build step. Use `pnpm --filter @hebbian/robot-use-client typecheck`
for a standalone typecheck. Restart running Node processes after source changes,
or use `pnpm dev:agent` for watch mode.

Start the station and enable the arm before running this example. See the [getting started guide](../docs/getting-started.md).

```ts
import { createStationConnection } from "@hebbian/robot-use-client";

const server = "http://127.0.0.1:8792";
const connection = createStationConnection(server, "agent");
connection.start();

let lease: Awaited<ReturnType<typeof connection.acquireLease>> | undefined;
try {
  // Motion exists only on the lease; throws LeaseHeldElsewhereError when someone else holds it.
  lease = await connection.acquireLease("my program");
  const actionSequenceId = await lease.startActionSequence({
    steps: [
      { arms: { arm: { movement: { kind: "joints", joints: [0, 0, 0, 0, 0] }, gripper_opening: 1 } } },
    ],
  });
  const result = await connection.waitForActionSequence(actionSequenceId);
  console.log(result.phase, result.steps);
} finally {
  try {
    await lease?.return();
  } finally {
    connection.close();
  }
}
```

Use targets appropriate for your arm and workspace. A completed action sequence
reached its planned targets; a stopped result explains why it ended. A dropped
connection stops the action sequence by default. See the
[control model](../docs/control-model.md) for lease and interruption behavior.

## Compile before running

Ordinary sequences compile automatically. Advanced callers can inspect first:

```ts
const outcome = await connection.compileActionSequence(source);
if (outcome.kind === "compiled") {
  console.log(outcome.artifact.compiled);
  const executionId = await lease.startCompiledSequence(
    outcome.artifact.compilation_id,
  );
  const result = await connection.waitForActionSequence(executionId);
  console.log(result.compilation_id, result.phase);
} else {
  console.log(outcome.diagnostics);
}
```

Here `source` has the same shape passed to `startActionSequence`, and `lease`
is a current lease handle. Compilation needs no lease; running still requires an
enabled arm and live admission checks. `connection.inspectCompilation(id)` retrieves
a retained artifact; save its JSON for portable export and load it with
`connection.importCompilation(artifact)`. An optional third argument to
`lease.startCompiledSequence` supplies `{ prior_execution_id, next_step }` for
explicit continuation. See [compilation and artifact semantics](../docs/control-model.md#compile-before-running).

## Model-facing tools

`createRobotTools` defaults to `action_sequence`, `preview_pose`, `observe` and
immediate `stop`. A single-arm sequence schema supplies the configured arm
implicitly. You can use these defaults without a mapping.
To restrict the model to pose and gripper actions, as the example agent does:

```ts
const robot = createRobotTools({ connection, lease, arm: "arm" })
  .mapTools(filterSequenceActions((action) => action !== "joints"));
```

`mapTools` receives typed capability definitions and returns a new view sharing
its connection and lease. Mappings compose in order, and definitions retain the
order returned by the mapping. Filtering sequence actions regenerates both the schema
and accepted arguments. The original view stays unchanged. Close the shared
views only when finished using them.

A custom transformation can change an `action_sequence` capability's `scope` to
`{ kind: "named", arms: ["left", "right"] }`. Named arms in a step are admitted
together, including their grippers; steps run sequentially.

To replace the default mapping, return a selection from
`allRobotToolCapabilities()`, including standalone `move_arms`,
`set_arm_gripper` or `rehome` when wanted. To restore the sequence-based preset,
return `defaultRobotToolMapping(allRobotToolCapabilities())` from the mapping.
Individual SDK methods are unchanged. Model arguments
use millimeters and degrees; station SDK arguments use meters and radians.

## The station connection

`createStationConnection(url, holder, { runAfter })` sends requests over HTTP and
receives state through the station's event stream. It reconnects immediately,
then backs off from 100 ms to a 1 s cap, using the last station event cursor. If
that cursor is outside retained history, the client resynchronizes from a snapshot;
it cannot recover every missed event. Command submissions retry transport failures
for up to 30 seconds with the same request ID and station instance ID, so the
station can recognize a repeated submission. A failed request does not by itself
establish whether the station acted; inspect the resulting state before deciding
what to do next. `waitUntilConnected()` rejects when the station refuses the connection.

- `connection.arm("arm")` has the commands that need no lease: `enable()`,
  `disable()`, `clearFault()`, `approveRecovery()` and `stop()`. `lease.arm("arm")`
  has the ones that move the arm (`moveToJoints(joints, maxJointSpeed?)`, `rehome()`,
  `moveToPose(..)`, `setGripper(opening)`, ...). Both are generated from the contracts
  (`armPowerCommands(send)` and `armMotionCommands(send)`, where `send` is any function
  taking a `StationCommand`) and run to the outcome; pass `{ waitForOutcome: false }`
  to resolve once the station accepts the command, and `{ signal }` to stop the arm on abort.
- The lease's `runCommand` and `runGroup` resolve once the command's record finished and the
  first state frame reflects it (no arm still has it active, and an arm reports it as
  its last command or a sampling arm was sampled at or after completion), so
  `state()` right after shows the arm at rest. The wait survives a reconnect for
  30 s. An aborted `AbortSignal` requests a stop in the background; rejection
  does not confirm that the arm has stopped. Observe the arm before continuing.
- `waitForState(predicate)` returns the first state frame that satisfies it, checking
  the latest first; `StationTimeoutError` after 30 s. `state()` waits up to 10 s for
  the first frame.
- `acquireLease(label, { requiredCameras, onInterruption })` gets the lease when no
  one holds it and returns the `Lease` handle (throwing `LeaseHeldElsewhereError`,
  which names the holder, otherwise); `takeLease` takes it from the holder;
  `lease.return()` gives it back and `onLeaseLost` reports a lease that ended
  otherwise. `revokeLease()` ends whoever holds it and resolves once the station
  reports no holder. The lease needs no powered arm. See
  [the control model](../docs/control-model.md).
- `lease.runCommand(arm, command, expectedRevision)` and `lease.runGroup(command, expectedRevisions)`
  are refused as `stale_revision` once an interlock was raised on the arm since that
  revision, and as `interlocked` while one is active.
- The lease reports a required camera (`lease.reportCamera`), stops an arm only while
  it is current (`lease.stopArm`) and stops the recording only while it is current
  (`lease.stopRecording`); `reportCamera` and `stopArm` throw `LeaseLostError`
  once the lease changes.
- `onRunRecord` delivers a model loop's run events, spans and resyncs, each
  record once; `recordRunEvent` and `recordRunSpans` report them.
- `createStationHttpClient({ stationUrl, mediaUrl })` lists cameras, fetches
  snapshots, creates phone invites and lists or downloads traces (gzip bytes as served).

Errors: `StationConnectionError`, `StationTimeoutError`, `StationRefusedError` (a
message the station could not act on), `LeaseHeldElsewhereError` and
`StationHttpError` (`status`, `body`). A refused or unfinished command is reported
in the answer's records, not thrown. Timeouts: 10 s to connect, 30 s for a reply, an
awaited state and every HTTP request. The station is `http://127.0.0.1:8792` and the
media service `http://127.0.0.1:8781`.

## Write a model loop

The [reference agent](../examples/typescript/agent/README.md) already supplies a
working loop. This section is for replacing that loop or integrating another
model provider; changing the model's output alone does not require it.

`createRobotTools` supplies model-facing tool declarations, observations and
execution for a loop you write. It uses the same Rust-derived catalog as the other SDKs.
Pass `descriptions: { observe: "Your description" }` when creating the tools to
customize model-facing text. `robot.tools` contains definitions, `await robot.context()`
provides robot facts for your own prompt, and `await robot.execute(calls)` returns
results with structured `data`, `text`, and outcome flags. Calls use `{ id, name,
arguments }` with parsed JSON arguments. Model inputs use `position_mm`,
`joints_degrees`, and `max_joint_speed_degrees_per_second`. `data` retains full SI
payloads; `text` and `modelData` provide the selected model view. Preview candidates contain only a `pose`; results follow input order
without IDs or labels. The adapter generates transport IDs internally. The station admits calls as one batch
and rejects the whole batch if two calls command the same arm resource. An
`observe` defaults to current state in `data` and labelled JPEGs in `cameras`.
Call it with `{ source: "last_sequence" }` to retrieve retained sequence images
and timing metadata instead. Automatic `await robot.observe()` calls always
return current views. If you own the provider integration, attach tool-result
`cameras` as image inputs alongside their labels. The reference agent does this
for you. See [sequence history](../docs/model-output.md#images-during-action-sequences)
for capture and retention limits.
For an existing connected `connection` and acquired `lease`:

```ts
const robot = createRobotTools({
  connection,
  lease,
  arm: "arm",
  descriptions: { action_sequence: "Execute ordered robot actions." },
});
const definitions = robot.tools;
const context = await robot.context();
const results = await robot.execute([
  { id: "1", name: "action_sequence", arguments: { steps: [{ gripper_opening: 0.5 }] } },
]);
console.log(definitions, context, results[0]?.data);
robot.close();
```

The full
reference loop, with Claude, OpenAI and Gemini, plan and notes tools, tracing, usage and
recording, is [`examples/typescript/agent`](../examples/typescript/agent); start the station, enable the
arm, then run it from the repository root:

```sh
export ANTHROPIC_API_KEY="your-key"   # or OPENAI_API_KEY, GEMINI_API_KEY with --provider
pnpm --filter @hebbian/agent start --provider anthropic -- arm "Describe the robot state. Keep the arm still."
```

For images, pass `--camera NAME` with
[the name registered at your station](../docs/cameras.md#connect-a-usb-camera).
Its flags (`--server`, `--media-server`, `--camera`, `--optional-camera`, `--on-interruption`,
`--record`, `--provider`, `--model`, `--reasoning-effort`) are the agent server's.
See [write your own loop](../docs/write-your-own-loop.md#the-typescript-reference-loop). The library
itself is independent of the model provider. Recording must be enabled on the station first;
recording continues across handover. The reference loop ends recording when its
task ends normally while it still holds the lease; cancellation leaves it running.

Acquire the lease on the connection before using `observeWhenReady()`, which
reports required cameras with its token. Call `robot.close()` when finished to
remove its lease-loss listener. Interrupted commands do not resume; the next model
turn receives a fresh observation and decides what to do.

Helpers for a loop: `observeWhenReady({ note, extraLines })` puts lines above the state text and
throws `ArmRecoveryEndedError` (arm faulted) or `LeaseLostError`;
`waitForArmRecovery()` waits for a recovered hold with no interlock and returns false when
the arm faulted; `execute(calls, revisions, wrap)` sends each arm's observed revision as
`expected_revision`, and lets `wrap(call, run)` surround each call
(for a trace span) and hand `run` the W3C `TraceContext` that parents the station's command span
(`runCommand` and `runGroup` take it as their last argument).

### Raw results and model views

The default formatter already gives the model concise state and motion outcomes.
Tool results retain complete station payloads in original SI units, available as
`result.data` without changing that formatter. Customize presentation only when
your task needs a different view; see
[model output](../docs/model-output.md#typescript-and-rust) for the language-specific
accessors and composition examples.

Compose output transforms before rendering, independently of tool selection:

```ts
import {
  renderOutput,
  selectOutputCameras,
  summarizeOutput,
} from "@hebbian/robot-use-client";

const taskView = robot.withOutputFormatter((output) =>
  renderOutput(selectOutputCameras(summarizeOutput(output), ["wrist"])),
);
```

The transforms return updated outputs in SI units. The renderer converts units
once and returns text and images. For custom payload selection, the package also
exports `conciseOutput`, `modelResponse` and `conciseResponse`.

## Package builds

The default package exports load compiled JavaScript and declarations in `dist/`.
`pnpm --filter @hebbian/robot-use-client build` builds these files; `pnpm pack`
also builds automatically through `prepack`. Only `dist/` is included in the
archive, and `publishConfig.exports` removes the workspace development override
from the packed manifest. The package remains private until an npm release is explicitly prepared.

Repository commands, Vite and TypeScript enable the `development` condition to
read current source. For a custom local script use
`pnpm exec tsx --conditions=development path/to/script.ts`. Plain Node consumers
use the compiled package by default. Run `pnpm test:package` to pack, install,
import and typecheck the SDK in a temporary consumer outside the workspace.

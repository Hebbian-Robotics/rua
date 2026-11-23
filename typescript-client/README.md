# TypeScript client

Use the TypeScript client to control a running station from a browser or Node.js.
Install a released version with `pnpm add hebbian-rua`.

## Try the workspace SDK

Use Node.js 24 and the pnpm version pinned in the root `package.json`.
From the checkout root, install dependencies with `pnpm install --frozen-lockfile`.
The runnable examples import the local workspace SDK.

To read state without moving the robot, start a station
([MuJoCo quickstart](../docs/simulation.md) or [hardware setup](../docs/getting-started.md)).
Save the following as `read-state.ts` in the checkout root:

```ts
import { createStationConnection } from "hebbian-rua";

const connection = createStationConnection("http://127.0.0.1:8792", "agent");
connection.start();

try {
  const state = await connection.state();
  console.log(state.arms);
} finally {
  connection.close();
}
```

Run it with `pnpm exec tsx read-state.ts`.

## Run an action sequence

Start the station and enable the arm before running this example. See the [getting started guide](../docs/getting-started.md).

```ts
import { createStationConnection } from "hebbian-rua";

const server = "http://127.0.0.1:8792";
const connection = createStationConnection(server, "agent");
connection.start();

let lease: Awaited<ReturnType<typeof connection.acquireLease>> | undefined;
try {
  // Motion exists only on the lease; throws LeaseHeldElsewhereError when someone else holds it.
  lease = await connection.acquireLease("my program");
  const actionSequenceId = await lease.startActionSequence({
    steps: [
      { arms: { arm: { goal: { kind: "joints", joints: [0, 0, 0, 0, 0] }, gripper_opening: 1 } } },
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
To restrict the model to pose and gripper actions:

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
  to resolve once the station accepts the command. Pass `{ signal }` to request cancellation
  of that operation on abort. Pass `operationId` from `newOperationId()` to keep
  a handle for `getOperation`, `lease.updateStreamGoal` or `lease.cancelOperation`.
- The lease's `runCommand` and `runGroup` resolve once the command's record finished and the
  first state frame reflects it (no arm still has it active, and an arm reports it as
  its last command or a sampling arm was sampled at or after completion), so
  `state()` right after shows the arm at rest. The wait survives a reconnect for
  30 s. An aborted `AbortSignal` requests cancellation of that operation in
  the background. Observe the arm to confirm its state before continuing.
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
  Direct camera requests use the station URL unless `mediaUrl` is set.
  `createRobotTools` uses the station’s observation route. Its `mediaUrl` option
  sets where it retrieves images during a sequence.

Errors: `StationConnectionError`, `StationTimeoutError`, `StationRefusedError` (a
message the station could not act on), `LeaseHeldElsewhereError` and
`StationHttpError` (`status`, `body`). A refused or unfinished command is reported
in the answer's records, not thrown. Timeouts: 10 s to connect, 30 s for a reply, an
awaited state and every HTTP request. The station defaults to `http://127.0.0.1:8792`.
The direct media address defaults to `http://127.0.0.1:8781`.

## Write a model loop

Start with the [TypeScript reference agent](../examples/typescript/agent/README.md)
for a CLI example with OpenAI, Anthropic and Gemini.

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
`cameras` as image inputs alongside their labels. See [sequence history](../docs/model-output.md#images-during-action-sequences)
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
[model output](../docs/model-output.md#choose-what-the-model-sees) for the language-specific
accessors and composition examples.

Compose output transforms before rendering, independently of tool selection:

```ts
import {
  renderOutput,
  selectOutputCameras,
  summarizeOutput,
} from "hebbian-rua";

const taskView = robot.withOutputFormatter((output) =>
  renderOutput(selectOutputCameras(summarizeOutput(output), ["wrist"])),
);
```

The transforms return updated outputs in SI units. The renderer converts units
once and returns text and images. For custom payload selection, the package also
exports `conciseOutput`, `modelResponse` and `conciseResponse`.

## Package builds

The package exports compiled JavaScript and declarations from `dist/`.
Build with `pnpm --filter hebbian-rua build`, or pack the SDK
from the checkout root. Packing runs the build automatically:

```sh
(cd typescript-client && pnpm pack --out /tmp/rua-client.tgz)
```

From your Node.js project, install the archive with
`pnpm add /tmp/rua-client.tgz` and import `hebbian-rua`. The
archive uses the compiled entry point. SDK tests in the workspace continue to
exercise the SDK under development. Run `pnpm test:package` to pack, install,
import and typecheck the SDK in a temporary consumer outside the workspace.

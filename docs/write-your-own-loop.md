# Write your own loop

You can run an agent without writing a loop. Start with the
[Python agent example](../examples/python/README.md#run-an-agent) or the
[TypeScript agent](../examples/typescript/agent/README.md), which already handle
observation, tool execution and stopping.

Use this guide when you want to build your own loop, connect another model SDK,
or drive the robot from a keyboard, gamepad or planner. If you only want to change
which tools or camera images a model sees, keep the existing loop: see
[tool selection](#with-a-model-four-pieces) and
[model output](model-output.md).

A loop reads the robot, decides, and commands the arm. The input can be a
keyboard, a gamepad, a vision model, a planner or any LLM SDK. Everything below is
in `hebbian_rua.client`; see [`examples/python/hello_robot.py`](../examples/python/hello_robot.py)
(no model) and [`examples/python/agent_quickstart.py`](../examples/python/agent_quickstart.py) (a model).
The client methods are asynchronous; await calls and use `async with` for control.
Start a station first ([getting started](getting-started.md) or
[simulation](simulation.md)).

For TypeScript, see the [client guide](../typescript-client/README.md#write-a-model-loop)
and the [TypeScript reference loop](#the-typescript-reference-loop).

The [Rust SDK guide](../sdk/README.md#model-facing-tools) covers the same tool
catalog and station batch execution. All three SDKs accept model-facing positions
in millimeters, angles in degrees, and speeds in degrees per second. Results use
the same units, rounding converted values to one decimal place with halfway values away from zero. Direct station-client values remain in
meters and radians.

The language interfaces reflect their host languages: Python and TypeScript
return tool results with error, fault, and interruption flags; Rust returns a
`RobotToolOutcome` variant and the structured station result. Python's context
uses snake_case, TypeScript's uses camelCase, and Rust returns the station state
alongside the shared conventions. Python and TypeScript can assemble a text and
camera observation; Rust applications attach images to provider messages.
The `observe` tool returns current state and images by default, or retained
sequence images with `{"source": "last_sequence"}`. Requested JPEGs are available
in `result.cameras`; Python and TypeScript provider adapters attach them to
model messages. Automatic observation helpers return current views only.
See [sequence history](model-output.md#images-during-action-sequences).

## Start with a working example

With [your SO-101 set up](getting-started.md), start with the Python SDK's
[read-only state check](../examples/python/README.md#read-your-arms-state).
For motion, [hello_robot.py](../examples/python/hello_robot.py) claims control,
checks the preview, moves to a target and checks the outcome before opening the
gripper. Choose a target appropriate for your arm and verify clearance before
running it. Use the same steps in your loop, supplying your own target.

`holding_lease` acquires the lease on the arms, keeps it on the connection (it ends if the connection drops, and the arms stop) and
returns it on exit. It yields a `Lease`, and every command that moves an arm lives on it:
`lease.arm("arm").move_to_pose(...)`. Power (`enable`, `disable`, `clear_fault`,
`approve_recovery`), `stop` and reads are on `station.arm("arm")`, which needs no lease.
The lease and the arm's power are independent: you may hold the lease while the arm is
disabled, and a move on a disabled arm is refused as `not_enabled` with the next step
(send `enable`). While you hold it, the viewer's manual controls and other programs
get `rejected` with `lease_held_elsewhere`; `arm.stop()` is accepted from anyone.
If another holder is there, `LeaseHeldElsewhere` is raised; pass `take=True` to
take the lease on purpose. See the [control model](control-model.md). The station stops every arm and waits for the control
loops to finish stopping before granting the new lease. If a stop fails, the
take raises an error and commands remain blocked until a successful retry. A
take does not disable the arms. If your process dies, the connection drops,
the lease is returned and the arm stops and holds.

`station.arm("arm").enable()` runs the preflight and holds the current pose. It returns a
`CommandResult`; check it. `disable` and `clear_fault` are on the same object.

## Commands and outcomes

| Call | Does |
| --- | --- |
| `lease.arm(name).move_to_pose(position, orientation=None, *, max_joint_speed=None)` | Tool position in metres, world frame. A `GraspOrientation(approach, jaw_yaw_degrees)` is an approach (`"down"`, `"forward"` or a vector) plus jaw yaw in degrees. Without one, the tool keeps its current orientation. |
| `station.arm(name).preview_pose(position, orientation=None)` | Moves nothing. Returns a `PoseProjection` whose outcome is `projected` with a pose, `within_tolerance`, pose errors and any constraint reason, or `unavailable` with a reason and no pose. It also includes the available jaw yaws. A request refusal returns `PreviewRefused`. Without an orientation the solver chooses one. |
| `lease.arm(name).move_to_joints(joints, *, max_joint_speed=None)` | Joint targets in radians. |
| `lease.arm(name).set_gripper(opening)` | 0 closed, 1 open. Stopping short can indicate contact or a blockage; it does not prove a grasp. |
| `station.arm(name).stop()` | Stop arm and gripper motion and hold with torque enabled. Needs no lease; does not end an agent run. |

Except for `preview_pose`, each returns a `CommandResult` with `outcome` (a `ResultOutcome`, which compares equal to its string), `detail` and `completed`.
`completed` means the planned motion finished; it does not always mean the
requested pose was reached. `result.was_limited` reports whether an accepted plan
was limited; inspect `result.record` for details. A refused command has no plan,
so check `completed` and `outcome` as well:

- `completed`: the planned motion finished, possibly at a limited target.
- `rejected`: the arm never moved; `detail` starts with the reason
  (`unreachable`, `arm_busy`, `not_enabled`, `faulted`,
  `invalid_target`, `lease_held_elsewhere`, `interlocked`, `stale_revision`). A pose no joints reach exactly is
  not necessarily rejected: the arm may move to the closest pose the solver
  found, and the record's `pose_limited` describes the shortfall. This is not a
  guarantee of the globally closest pose.
- `tripped`: a safety limit stopped the arm and it holds where it was. Look at the
  cameras and state before retrying.
- `timed_out`: the move did not finish in time.
- `stopped_short`: the arm was not within the settled tolerance of the target after
  the settle timeout (`settle_timeout_seconds`). `achieved.joint_errors_radians` gives each joint's remaining
  error and `target_status` is `stopped_short`, not `reached`. Judge from the frames
  whether it is close enough, then repeat or adjust the move.
- `superseded`: a new move replaced this one while it was only settling; `achieved`
  holds the error at that moment.
- `faulted`: a hard fault released the motors. Stop the loop; a person clears it
  (`hebbian-robot-use clear-fault`).

The station also reports `stopped` (after a stop) and `failed` (a command such as
enable that never moved the arm). `result.record` is the typed station answer.
For an accepted command, `result.record.completion` holds its `finished_at`,
`outcome`, and optional `detail` together; it is `None` until the command ends.

### Compile before running

Ordinary action-sequence tools compile automatically. Advanced callers can
compile and inspect a program before acquiring control, save it for later use,
or continue an execution explicitly. See the
[compilation guide](control-model.md#compile-before-running) for the Python
example, artifact lifetime and continuation rules.

### preview_pose or move_to_pose

`move_to_pose` is the move. `preview_pose` is a free check before it: use it to see
whether a target is reachable and which jaw yaws work. It accepts a position alone
and lets the solver choose the orientation, which `move_to_pose` does not do (no
orientation means "keep the current one"). Pass the same explicit orientation to
both when checking a particular move. A preview uses the configured model-based
self-collision and surface checks, but cannot detect objects or other arms and
is not a certificate of physical clearance.

`WorldPositionMeters` and `JointSpeedRadiansPerSecond` are optional named types
for values passed to the client. They preserve compatibility with lists and floats;
they do not validate values. The station remains responsible for limits.

## Reading state

`arm.state()` returns an `ArmState`:

```python
snapshot = await arm.state()
snapshot.mode  # ArmMode: disabled, holding, moving, faulted
snapshot.measured_joints  # radians
snapshot.tool.position  # [x, y, z] metres; `tool` is None before a pose exists
snapshot.tool.quaternion  # [x, y, z, w]
snapshot.tool.orientation  # approach, degrees_from_down, jaw_yaw_degrees
snapshot.gripper_opening  # 0 closed to 1 open; None without a gripper
snapshot.fault  # str or None
```

Camera frames are separate: `await station.camera_names()` and
`(await station.camera_jpeg(name)).jpeg`. Frames and state are not time-synchronized.

## With a model: four pieces

`RobotTools(station, lease, arm_name, camera_names)` supplies
everything the model needs; the loop and the SDK call are yours.

| Piece | Call |
| --- | --- |
| Robot facts for your own prompt | `await robot.context()` |
| Observe: state as text and a JPEG per camera | `await robot.observe()` returns a `RobotObservation` (`.text`, `.cameras`) |
| Tools: `RobotToolSpec` with `name`, `description`, `parameters_schema` (plain JSON schema) | `robot.tools` |
| Execute: carry out one turn's calls | `await robot.execute([RobotToolCall(call_id, name, arguments)])` returns `RobotToolResult`s (`.data`, `.text`, `.cameras`, `.is_error`, `.halt`) |

For OpenAI, Anthropic and Gemini, use the adapters from `hebbian_rua.client.llm.openai`,
`hebbian_rua.client.llm.anthropic` and `hebbian_rua.client.llm.gemini`. They supply native `.tools`,
`.observe()` messages and `.execute(response)` results. Append `turn.messages`
to your conversation and stop on `turn.should_stop` or empty `turn.results`.
See the [three-provider example](../examples/python/agent_quickstart.py).
For other SDKs or mixed toolsets, use the provider-neutral types above and route
calls yourself. Stop on a result's `halt` or `interrupted` flag.
For interruptions, `capture_when_ready(robot)` waits for arm and required-camera
recovery and returns a fresh capture with the arm's revision. Pass that revision
as `expected_revision` when executing tools; a command whose arm was interrupted
since is refused as `stale_revision`. `station.ensure_recording()`
starts or joins a recording; use `lease.stop_recording()` when the task
ends so a handover cannot stop the new holder's recording. In TypeScript,
`robot.observeWhenReady()` returns the capture and each arm's revision, the recording
client provides `ensure()`, and `lease.stopRecording()` stops it only while the lease is current.

The API docstrings in [station_client.py](../hebbian_rua/client/station_client.py)
and [robot_tools.py](../hebbian_rua/client/llm/robot_tools.py)
describe behavior at the methods themselves.

The library’s default model tools are `action_sequence`, `preview_pose`, `stop`
and `observe`. Its single-arm preset allows pose, joints and gripper actions.
Both reference examples filter out joints. The Python example adds `finish`;
the TypeScript example adds `finish`, `update_plan` and `update_notes`. These
application tools are not supplied by the robot library.

Each sequence contains ordered steps; movement and gripper in a step
start together. A single step expresses a single action. Arm selection is
implicit in the single-arm model schema.

Configure available actions in code; the same configuration constrains both the
schema and argument parsing:

```python
from hebbian_rua import RobotTools, filter_sequence_actions

robot = RobotTools(station, lease, "arm").map_tools(
    filter_sequence_actions(lambda action: action != "joints")
)
```

Mappings compose in order: each mapping receives the preceding mapping's
capabilities, and tool definitions preserve the returned order. `map_tools`
transforms immutable capability definitions and returns a view sharing
the connection, lease and output formatter. It composes with `with_output_formatter`; neither
changes the original view. TypeScript provides `mapTools` and
`filterSequenceActions`; Rust provides `map_tools` with an owned `FnOnce` mapping.

For multiple arms, replace the sequence capability's scope with
`NamedArmSequence(("left", "right"))`. Each step then has an `arms` map; every
participating target is planned before the step is admitted together. TypeScript
uses `{ kind: "named", arms: ["left", "right"] }`; Rust uses `SequenceScope`.

To replace the default mapping, select capabilities from
`all_robot_tool_capabilities()` (TypeScript: `allRobotToolCapabilities()`), which
also includes standalone movement, gripper and rehome tools. The default mapping
selects sequences, previews, observation and immediate stop. Individual SDK
methods remain available regardless of the model catalog. Model speeds use
degrees per second and are clamped to the profile maximum; SDK station arguments
use SI units.

The model-facing `preview_pose` accepts a `poses` list, even for one candidate.
Each candidate contains only a `pose`; the SDK generates internal IDs. For example:

```json
{"poses":[{"pose":{"position_mm":[200,0,150]}}]}
```

The result contains `results` in input order, each with its individual answer and
no transport IDs or labels. Candidates are independent checks, not steps in a path.

The Python and TypeScript clients can start, monitor and cancel action sequences. An
action sequence defaults to stopping when its submitting connection disconnects; choose
`continue` to let the station finish it. Stop, a lease take or revoke, and
interlocks (arm or required-camera interruptions) can still end execution: the
station refuses to start an action sequence while the arm has an interlock and stops it
between steps once the arm's revision moves. ActionSequence state includes
`requested_steps`, the current phase, completed step records, and the reason the
action sequence stopped. Subscribers receive the current state after reconnecting.

For example, with an enabled `arm` arm, submit two
joint targets from Python:

```python
import asyncio

from hebbian_rua.client.contract.station_api import (
    ActionSequenceJointsMovement,
    ActionSequenceArmAction,
)
from hebbian_rua.client.station_client import StationClient


async def main():
    async with StationClient() as station:
        async with station.holding_lease("action sequence example") as lease:
            result = await lease.arm("arm").run_action_sequence(
                [
                    ActionSequenceArmAction(
                        movement=ActionSequenceJointsMovement(joints=[0.1, 0, 0, 0, 0]),
                        gripper_opening=1.0,
                    ),
                    ActionSequenceArmAction(
                        movement=ActionSequenceJointsMovement(joints=[0.0, 0, 0, 0, 0])
                    ),
                ]
            )
            print(result.phase, result.steps)


asyncio.run(main())
```

Use joint positions that are appropriate for your arm and workspace. A completed
action sequence reports `completed`; a stopped action sequence explains why it stopped.

### Reasoning effort

The TypeScript reference agent accepts `low`, `medium` or `high`, defaulting to
`high`. Its provider adapters translate that setting to the provider’s effort or
thinking-level field. Use `--reasoning-effort` on the CLI or the viewer setting.
Supported models must accept the selected provider’s configuration; a provider
SDK accepting a value does not imply every model supports it.

The Python example sends provider-specific configuration in its own code; it does
not expose the TypeScript agent’s shared reasoning-effort flag. Inspect the
[example](../examples/python/agent_quickstart.py) when customizing it.

## The TypeScript reference loop

`examples/typescript/agent/` (`@hebbian/agent`) is the reference loop:
Claude, OpenAI and Gemini through their official SDKs (`@anthropic-ai/sdk`, `openai`,
`@google/genai`), plan and notes tools, `finish`, recovery waits, run events, trace
spans and token usage in the viewer, optional recording, and cancellation. The command line
below is a thin wrapper over `runAgent`, the function the agent server calls in-process. Keys come from `ANTHROPIC_API_KEY`,
`OPENAI_API_KEY` or `GEMINI_API_KEY`, in the environment or a `.env` file in the working
directory. From the repository root, with a station running and the arm enabled:

```sh
pnpm install --frozen-lockfile
export ANTHROPIC_API_KEY="your-key"
pnpm --filter @hebbian/agent start --provider anthropic -- arm "Describe the robot state. Keep the arm still."
```

For images, pass `--camera NAME` using
[the name registered at your station](cameras.md#connect-a-usb-camera).
Other flags: `--optional-camera`, `--on-interruption wait_for_approval|return_lease`, `--record`, `--model` (a current model; older ones without adaptive thinking are not supported) and
`--media-server`. The loop stops the arm and exits on SIGINT or SIGTERM, leaving a recording
running. As a library, `runAgent(config, { signal, onEvent })` takes a typed config (provider, model,
arm, cameras, task, station and media URLs, API key) and resolves to `completed`, `stopped` (the
signal aborted: the arm is stopped) or `failed`; it never rejects. The robot profile's speed limits apply to every motion. Its pieces are `providers/` (one conversation class per SDK), `tools.ts` (plan, notes,
finish), `execution.ts` (a turn's tool batch), `observation.ts`, `tracing.ts` and `loop.ts`.

The loop captures fresh state and camera images after physical actions. An
explicit current `observe` supplies its own capture, reused for the next state
revision and memory view. A history-only `observe`, plan updates and pose previews
return results without another capture. Successful plan and notes calls return
a short acknowledgement; requested history remains in its tool result. The
current observation carries the updated memory and state once.

## Without a model

Skip `RobotTools` and call the arm directly: read with `station.arm(name)` and move with
`lease.arm(name)`. For a keyboard or gamepad, map each key
or stick to a small step from `arm.state().tool.position` and call
`move_to_pose` on the leased arm; for vision, turn a detection into a position in the robot frame.
[`hello_robot.py`](../examples/python/hello_robot.py) shows connect, preview, move and
check. Keep steps small, check every outcome, and call `station.arm(name).stop()` on exit or error.

## Run your program from the viewer

A loop needs only the SDK and the station address: run it from any machine, with
its own model keys, and it claims control through the client. The robot station
does not launch loops.

The viewer's run panel starts loops through the optional **agent server**
(`pnpm start:agent`, or `pnpm dev`, which runs it beside Vite). The server runs the
TypeScript loop in-process: it calls `runAgent` from `@hebbian/agent` directly, with an
`AbortController` per run, and does not spawn a child process. It holds the
model keys (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GEMINI_API_KEY`), runs one
loop at a time, keeps the last 200 output lines and a JSON record per run under
`.agent-runs/`, and stops a run by aborting it, which stops the arm. Each new run has a `metadata.json` summary and an append-only `output.jsonl` with timestamped output. The viewer shows the last 200 lines; the file retains the full output and can contain task or model-generated content. Writes are buffered by the operating system, so a crash can lose recent output. Storage failures appear in the viewer. The full output is available as JSONL at `GET /api/runs/{id}/output`. Without the server the viewer still shows the live state, cameras and
manual control; the run panel explains how to start the agent server. The CLI is a thin wrapper over the same `runAgent`.

A loop in another language runs from its own terminal against the station; the agent server
only hosts the TypeScript loop.

Other settings: `HEBBIAN_STATION_URL` (default `http://127.0.0.1:8792`, the
station the loop drives), `HEBBIAN_MEDIA_SERVER` (the camera service the loop reads), `AGENT_PORT`
(8793), `AGENT_HOST` (127.0.0.1), `AGENT_RUNS_DIR` and `AGENT_ALLOWED_ORIGINS`
(comma-separated viewer origins for CORS; by default the origin of `HEBBIAN_STATION_URL`
and the Vite dev server). If you set `AGENT_ALLOWED_ORIGINS`, include the station origin
and any other viewer origins you need. The server has no authentication; bind it to a trusted network only. In
the viewer, Settings sets its address (default: this host, port 8793; build-time
`VITE_AGENT_URL`).

Start the simulator in another terminal as shown in the
[simulation guide](simulation.md#1-run-a-simulated-station).

Stopping a run in the viewer aborts the in-process loop, which stops the arm. See
[recording](recording.md) to capture it.

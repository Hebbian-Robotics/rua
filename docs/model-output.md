# Choose what the model sees

Most applications can use the default output unchanged. The reference agents
and provider adapters already give the model concise robot state, motion outcomes
and configured camera images. You do not need to write a formatter.

Start with the [Python agent example](../examples/python/README.md#run-an-agent)
or the [TypeScript agent](../examples/typescript/agent/README.md).
Return to this guide if you want to change what the model sees, such as selecting
a camera for a task or including more diagnostic detail. Customizing presentation
does not require writing your own robot control loop.

- **Use the defaults:** read [what the model sees](#model-views-in-the-adapters).
- **Inspect full results in your application:** see [data and logging](#application-data-and-logging);
  you do not need to change the model's formatter.
- **Customize presentation:** use [full output](#full-output) or
  [your own formatter](#your-own-formatter).
- **Control the robot directly:** see [write your own loop](write-your-own-loop.md).

## Model views in the adapters

`RobotTools` keeps full station responses and formats a concise view for the
model. The same view works with the OpenAI, Anthropic and Gemini adapters.

By default, observations show each arm once, with measured joints, tool pose,
gripper state, faults and interlocks. Motion results keep outcomes, achieved
poses and limits while omitting checkpoints, trace metadata and rendering
geometry. Model-facing `target_status` is `limited` when execution reached a
pose or path endpoint adjusted by the planner; raw station records keep their
planned-target status. Sequence results show their phase, requested/recorded step
counts, a compact outcome for each step, and full concise details for the final
step. Model views use rounded
millimetres and degrees; model tool inputs use the same units, with speeds in
degrees per second. Direct station inputs use metres and radians. Joint
velocities and efforts are retained in full output for diagnostics, rather than
included in the concise state. Full output preserves velocities in radians per
second and efforts in the hardware profile's units; efforts are not necessarily
torque in newton-metres.

## Images during action sequences

Model-facing tools sample their configured cameras independently of recording.
They request snapshots at sequence start and end, on observed step changes, and
every 250 ms during execution, including a sequence with only one motion. Capture
runs separately from command execution; slow or unavailable cameras leave gaps
instead of delaying or stopping motion. Boundary images are best effort: a fast
motion can finish before the first snapshot arrives, and an in-flight capture
can cause a step-boundary request to be skipped. Timestamps describe client
retrieval intervals, not synchronized robot state or guaranteed camera exposure.

Each tool instance retains only its latest sequence, shared across mapped views,
with limits of 64 images and 16 MiB of JPEG data. It preserves the first retained
image and the most recent images when discarding history. Images expire five minutes after
the sequence capture finishes. A selected view contains at most eight images and
2 MiB, prioritizing recent evidence when the byte budget cannot fit everything.
Images larger than 2 MiB are skipped. Camera order rotates so a slow first camera
does not permanently prevent another camera from being sampled. Each capture
batch has a 500 ms deadline; requesting the end snapshot can add up to 500 ms
after execution finishes, without delaying the movement itself.

Automatic observations contain current state and current camera images only.
The model can use the same `observe` tool to choose a source:

```json
{"source": "current"}
```

```json
{"source": "last_sequence"}
```

Omitting `source` selects `current`. Both sources accept `wait_seconds` from zero
to ten. A history request returns sampled images from the latest sequence,
including its call identifier, camera names, retrieval intervals, missed samples
and retention notices. It does not run motion or require a fresh camera frame.
If no history was captured or it expired, the tool returns an error explaining
that history is unavailable. Requests do not consume history; the model can
review it again until it expires or a new sequence replaces it.

Python and TypeScript return requested JPEGs in `result.cameras`. Their provider
adapters attach labelled images after the correlated tool responses, so the
history reaches the model without requiring another observation. Historical
labels identify the sequence call, camera, elapsed time and retrieval interval;
plain camera names identify current views. `select_output_cameras` /
`selectOutputCameras` also filters historical images in tool results.
For application access, `robot.sequence_images()` in Python and
`robot.sequenceImages()` in TypeScript return the selected retained history.
History is local to the SDK instance and is lost when that process exits;
direct station sequence calls do not enable this model-tool capture.

Rust uses the same tool sources, capture and retention policy. Configure
`RobotTools::new(&lease, "arm").with_cameras(vec!["wrist".into()])`.
An `observe` result contains the requested labelled JPEGs in `result.cameras`;
the application attaches them to its provider messages. Rust does not supply
provider adapters. `tools.sequence_images()` also exposes retained history for
application-owned workflows.

## Full output

Pass a different view of your existing tools to any provider adapter:

```python
from hebbian_rua import full_formatter
from hebbian_rua.client.llm.openai import OpenAIRobotTools

model_tools = OpenAIRobotTools(robot.with_output_formatter(full_formatter))
```

`robot` is your existing `RobotTools` instance. `full_formatter` shows all fields
in their original units and precision. It includes configured observation
cameras, notes and camera availability/timing information.

## Your own formatter

One function handles both observations and tool results. It receives full data,
not the default summary. Branch on `output.kind` or `output.call.name` when you
want different presentation for a particular tool.

```python
from hebbian_rua import (
    ModelContent,
    RobotOutput,
    render_output,
    select_output_cameras,
    summarize_output,
)
from hebbian_rua.client.llm.openai import OpenAIRobotTools


def format_for_task(output: RobotOutput) -> ModelContent:
    output = summarize_output(output)
    output = select_output_cameras(output, {"wrist"})
    return render_output(output)


model_tools = OpenAIRobotTools(robot.with_output_formatter(format_for_task))
```

`ModelContent` carries text and camera JPEGs for observations and tool results.
Observe results carry the current images or requested sequence history; the
provider adapters attach these images to their messages. Observation inputs also
include the selected arm, notes, unavailable optional cameras and camera retrieval
intervals. Preserve information your task needs to interpret failures or stale views. Formatters should be pure functions;
callback exceptions propagate to the caller without retrying robot commands.

`with_output_formatter` returns a new view sharing the station and lease. It leaves the
original tools unchanged and replaces any previous formatter. Compose ordinary
functions inside your formatter when needed. Camera filtering changes model
content; it does not change configured required-camera checks or capture.
Formatting cannot change command execution or error, halt and interruption flags.
Close the shared station only after all views have finished using it.

### Compose transforms before rendering

`summarize_output` and `select_output_cameras` accept and return `RobotOutput`.
Use `dataclasses.replace` for your own Python transforms; TypeScript uses object
spread. Transforms return updated values and leave their inputs untouched. Keep
payloads in SI units until `render_output`, which converts to model units and
rounds. `render_output(output, units="si")` retains original units and precision.

`default_formatter` is `render_output(summarize_output(output))`;
`full_formatter` renders the original output with `units="si"`. Camera selection
also selects retrieval intervals in Python, while keeping availability notices.
It does not change capture or required-camera checks.

TypeScript exports the same pieces as `summarizeOutput`, `selectOutputCameras`
and `renderOutput`; use `renderOutput(output, { units: "si" })` for exact SI output.
No pipeline object or separate intermediate type is needed.

## Application data and logging

With the lower-level SDK, `await robot.execute_one(call)` returns a
`RobotToolResult`; provider adapters also expose these as `turn.results`:

- `result.data`: complete station payload in original units and precision,
  or the original error text.
- `result.text`: the selected model-facing text.
- `result.model_data`: the selected text parsed as JSON when possible.
- `result.cameras`: labelled JPEGs explicitly requested by an observe tool call.

`await robot.observe()` returns `RobotObservation`, whose `data` is the full
station state, with selected model text and images in `text` and `cameras`.
Use `capture()` when you also need the original camera capture metadata.

## TypeScript and Rust

TypeScript keeps the same separation: `result.data` and `observation.data` retain
full station data in metres and radians. `result.text` is the selected model text,
and `result.modelData` parses that text as JSON when possible.

```typescript
import {
  fullFormatter,
  renderOutput,
  selectOutputCameras,
  summarizeOutput,
} from "@hebbian/robot-use-client";

const detailed = robot.withOutputFormatter(fullFormatter);
const taskView = robot.withOutputFormatter((output) =>
  renderOutput(selectOutputCameras(summarizeOutput(output), ["wrist"])),
);
```

Like Python, `withOutputFormatter` replaces the previous formatter, shares the connection
and lease, and leaves the original view unchanged. A formatter receives an isolated
copy of the complete payload. It cannot change execution flags or stored data.
Observation and result formatters can select text and camera images.
Close the shared tools only after all views are finished. Formatter errors propagate
after execution; they do not cause a command retry.

Rust retains full payloads in `RobotToolOutcome::Returned`, `Incomplete` and
`Interrupted`, preserving outcome matching independently of presentation.
`result.data()` returns the full payload or error/refusal information;
`result.model_data()` produces the default concise model view.
For custom presentation, compose ordinary functions on the full data:

```rust
let raw_data = result.data(); // Full SI payload; also suitable for logging.
let model_data = result.model_data(); // Concise millimetres/degrees view.
let custom_view = my_formatter(result.data());
```

Rust exposes the same payload stages as owned JSON transformations. `concise_output`
selects fields without changing SI units; `model_response` converts units and rounds.
Keep serialization at the provider boundary:

```rust
use hebbian_robot_use_sdk::{ConciseOptions, concise_output, model_response};

let selected = concise_output(result.data(), ConciseOptions::default());
let model_payload = model_response(selected);
let model_text = serde_json::to_string(&model_payload)?;
```

Python exports these payload functions as `concise_output`, `model_response` and
`concise_response`; TypeScript exports `conciseOutput`, `modelResponse` and
`conciseResponse`. Use them for custom payload transforms, or use the whole
`RobotOutput` helpers when carrying notes and camera images. Keep unit conversion
at the final stage. Each SDK retains the original result independently.

## Reference: fields and unit conversion

Rust, Python and TypeScript build the same concise model view in two layers, and
the CLI prints it by default (`--raw` prints the full station record):

1. **Concise:** a station state keeps `robot`, `joint_names`, `max_joint_speed`,
   `surface_guard` and, for each arm, `arm`, `mode`, `status`, `fault`,
   `interlocks`, `recovery_issue`, `revision`, `sampled_at_unix_seconds`,
   `measured_joints`, `tool` and `gripper_opening`. An action sequence becomes
   its `arms`, `phase`, `requested_step_count`, `recorded_step_count` and
   `steps`, with a compact outcome for each earlier step and full concise details
   for the final step. Everywhere else `checkpoints`, `trace_context`, `model_points`,
   `accepted_at` and `finished_at` are dropped. Internal `id`, `command_id`,
   `group_id`, `sequence_id` and `action_sequence_id` fields are also omitted;
   the tool layer tracks these without asking the model to copy them. Arm and
   camera names remain available to identify resources.
2. **Units:** known fields convert to millimetres and degrees and round; nulls
   and empty checkpoints are dropped; quaternion components round to four decimals (less than 0.012° of
   orientation error after normalization); approach vectors and jaw yaw stay exact.

Observations leave out the station `description` and `lease` because the model is
given the conventions separately. The CLI keeps both in `state`, since an agent
there has no other way to learn the frames and who holds the lease (Rust
`ConciseOptions { keep_station_context }`, Python `keep_station_context=True`,
TypeScript `keepStationContext: true`).

The specification is `tests/fixtures/model-concise.json` (concise layer and full
view) and `tests/fixtures/model-response.json` (units and rounding); each language
runs both. Change a fixture and all three implementations must follow.

The hand-written implementations are:

| SDK | Field selection and units | Observation rendering |
| --- | --- | --- |
| Python | `hebbian_rua/client/llm/response_format.py` | Same module |
| TypeScript | `typescript-client/src/response-format.ts` | `typescript-client/src/output-format.ts` |
| Rust | `sdk/src/response_format.rs` | The application assembles camera content |

When changing shared output behavior, update the affected implementations and
independent fixture expectations together. Preserve failure details and observation
context; fixtures specify behavior rather than generating formatter code.

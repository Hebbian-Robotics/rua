# Choose what the model sees

The SDK separates the full station payload from the concise text and camera
images sent to the model. Start with the
[Python agent example](../examples/python/README.md#run-an-agent).

## Model views in the adapters

The concise view keeps measured arm state, command outcomes and limit details.
A `limited` result can mean the arm reached an endpoint adjusted by the planner.
Model views use millimetres and degrees. Direct station calls and full payloads
use metres and radians. See the
[tool input units](../schemas/robot-tool-input-units.json) for argument conversion.
Motion JSON in the [CLI](../cli/README.md) uses the same unit fields. The
CLI exposes state and camera snapshots as separate commands. The example
agent loops can attach observations to tool results and retain sequence
images across calls.


## Images during action sequences

When a model tool starts an action sequence with cameras configured, the
station samples its media companion alongside motion. Slow or unavailable
cameras can leave gaps. Retrieval times are measured at the station.
Capture is independent of recording and does not delay motion.

Use `observe` with `source: "last_sequence"` for the latest captured sequence
or `source: "current"` for a fresh observation. Provider adapters attach
labelled images to their tool replies. The SDKs expose the last retrieved
history through `sequence_images()` in Python and Rust or `sequenceImages()`
in TypeScript.

An HTTP client can opt in by including `capture_cameras` when starting
`POST /action_sequence`. Read `GET /action_sequence/{id}/images` with the
returned execution ID, including after reconnecting. The route returns
`null` when capture is still pending or the history is unavailable.
The station keeps up to eight active or completed histories. A completed
history expires after five minutes or when evicted to make room. Each
response includes up to eight images and 2 MiB of JPEG data.

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

A formatter receives full data for both observations and tool results.
Branch on `output.kind` or `output.call.name` to tailor the model view.

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


### Compose transforms before rendering

Summarize or select fields before rendering converts units. A formatter can
change the model's text and image selection. The configured capture, camera
requirements, lease and command execution belong to the underlying tools.
A formatter error reaches the caller after execution without retrying the
command.

## Application data and logging

Log `result.data` or `observation.data` when you need full station payloads.
Use `text` and `cameras` to inspect the model-facing view.
In Python, `capture()` includes the original camera capture metadata.

## TypeScript and Rust

TypeScript offers `withOutputFormatter`, `fullFormatter`, `summarizeOutput`,
`selectOutputCameras`, and `renderOutput`. See the
[output helpers](../typescript-client/src/output-format.ts).

Rust applications assemble provider messages. Use `result.data()` for raw
outcomes, `result.model_data()` for the concise view, and the
[formatting helpers](../sdk/src/response_format.rs) for custom presentation.

## Reference: fields and unit conversion

See the shared [field-selection fixture](../tests/fixtures/model-concise.json)
and [unit-conversion fixture](../tests/fixtures/model-response.json) for exact
output. The formatter implementations live in
[Python](../hebbian_rua/client/llm/response_format.py),
[TypeScript](../typescript-client/src/response-format.ts), and
[Rust](../sdk/src/response_format.rs).

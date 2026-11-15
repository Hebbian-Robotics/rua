# Choose what the model sees

`RobotTools` keeps full station responses and formats a concise view for the
model. The same view works with the OpenAI, Anthropic and Gemini adapters.

By default, observations show each arm once, with measured joints, joint velocities and efforts, tool pose,
gripper state, faults and interlocks. Motion results keep outcomes, achieved
poses and limits while omitting checkpoints, trace metadata and rendering
geometry. Sequence results show their phase, requested/recorded step counts and
the last step instead of repeating the whole history. Model views use rounded
millimetres and degrees, with velocities in degrees per second. Effort readings
keep the hardware profile's units; they are not necessarily torque in newton-metres.
Full output preserves velocities in radians per second. Tool inputs still use
metres and radians.

## Full output

Pass a different view of your existing tools to any provider adapter:

```python
from hebbian_rua import full_formatter
from hebbian_rua.openai import OpenAIRobotTools

model_tools = OpenAIRobotTools(robot.map_output(full_formatter))
```

`robot` is your existing `RobotTools` instance. `full_formatter` shows all fields
in their original SI units and precision. It includes configured observation
cameras, notes and camera availability/timing information.

## Your own formatter

One function handles both observations and tool results. It receives full data,
not the default summary. Branch on `output.kind` or `output.call.name` when you
want different presentation for a particular tool.

```python
from hebbian_rua import ModelContent, RobotOutput, default_formatter
from hebbian_rua.openai import OpenAIRobotTools


def format_for_task(output: RobotOutput) -> ModelContent:
    content = default_formatter(output)
    if output.kind == "observation":
        # Keep the default state text, but send only the camera this task needs.
        cameras = {
            name: jpeg for name, jpeg in content.cameras.items() if name == "table"
        }
        return ModelContent(content.text, cameras)
    return content


model_tools = OpenAIRobotTools(robot.map_output(format_for_task))
```

`ModelContent` carries text and, for observations, camera JPEGs. Tool results
currently support text only; returning camera images for a result raises an
error. Observation inputs also include the selected arm, notes, unavailable
optional cameras and camera retrieval intervals. Preserve information your task
needs to interpret failures or stale views. Formatters should be pure functions;
callback exceptions propagate to the caller without retrying robot commands.

`map_output` returns a new view sharing the station and lease. It leaves the
original tools unchanged and replaces any previous formatter. Compose ordinary
functions inside your formatter when needed. Camera filtering changes model
content; it does not change configured required-camera checks or capture.
Formatting cannot change command execution or error, halt and interruption flags.
Close the shared station only after all views have finished using it.

## Application data and logging

With the lower-level SDK, `await robot.execute_one(call)` returns a
`RobotToolResult`; provider adapters also expose these as `turn.results`:

- `result.data`: complete station payload in original SI units and precision,
  or the original error text.
- `result.text`: the selected model-facing text.
- `result.model_data`: the selected text parsed as JSON when possible.

`await robot.observe()` returns `RobotObservation`, whose `data` is the full
station state, with selected model text and images in `text` and `cameras`.
Use `capture()` when you also need the original camera capture metadata.

Previously, `result.data` parsed the formatted text. Use `result.model_data` for
that behavior; use `result.data` for complete application data. The station
client itself and the CLI continue returning their existing full responses.

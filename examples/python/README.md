# Python examples

Start RUA with your robot or simulator connected
([getting started](../../docs/getting-started.md)). This directory is an
independent uv project using the SDK from this checkout:

```sh
cd examples/python
uv sync --locked
```

For your own agent project, use `uv add "hebbian-rua[openai,anthropic,gemini]"`,
or choose just the provider extras you need.

## Run an agent

The [agent example](agent_quickstart.py) supports OpenAI, Claude and Gemini.

Set `OPENAI_API_KEY`, `ANTHROPIC_API_KEY` or `GEMINI_API_KEY` for your provider.

Pass `--model` to select a model available in your account. Anthropic and Gemini
require it. Set `ANTHROPIC_MODEL` or `GEMINI_MODEL` for the corresponding example:

```sh
uv run agent_quickstart.py "Wave hello" --provider openai --camera table
uv run agent_quickstart.py "Wave hello" --provider anthropic --model "$ANTHROPIC_MODEL" --camera table
uv run agent_quickstart.py "Wave hello" --provider gemini --model "$GEMINI_MODEL" --camera table
```

Both reference agents use the model selected by the station profile. The
physical-arm setup defaults to `so101-soft-gripper-camera-manual`, our SO-101
with TPU fingers and the HBVCAM-W20217 V22 wrist camera. Keep your arm-specific
port and calibration in a local profile extending that preset. For a stock
SO-101, extend `so101-manual` instead. The station loads the URDF for control
and collision checks and serves the same model to the viewer; the agent does
not load a separate robot model.

These commands run from this directory. Use `--help` for the arm, station and
media endpoints. The example uses the provider's official async SDK directly.
Adapters supply `.tools`, a model-ready `.observe()` message, and
`.execute(response)`, which returns `.messages`, `.results` and `.should_stop`.
Append the messages without rebuilding them: they preserve reasoning and
signatures. The example adds a local `finish` tool to report the task outcome
and end the loop. It must be called alone; a mixed batch is rejected before
any robot command runs. Faults and uncertain outcomes also stop the loop.
An ordinary final response without tool calls remains supported.

Adapters accept complete, non-streaming responses and handle the robot function
calls in that response. Truncated, blocked or otherwise incomplete responses
raise an error before execution. Gemini expects one candidate and manual
function calling; the example disables automatic function calling explicitly.
For mixed toolsets, use `RobotTools` and the `RobotToolCall`/`RobotToolResult`
types to route calls yourself. The adapters don't run a model, own conversation
history or enable the arm.

See [camera setup](../../docs/cameras.md) for feeds and the web preview.
For lower-level SDK calls without a model, see [hello_robot.py](hello_robot.py).

## Customize the model view

The example already supplies concise robot state, motion outcomes and configured
camera images. Keep the default formatter unless your task needs a different view.
This optional customization changes presentation; the agent loop stays the same.

Tool selection and output formatting are independent. Starting with the example's
`robot`, compose transforms and install the resulting formatter:

```python
from hebbian_rua import (
    ModelContent,
    RobotOutput,
    render_output,
    select_output_cameras,
    summarize_output,
)


def task_formatter(output: RobotOutput) -> ModelContent:
    output = summarize_output(output)
    output = select_output_cameras(output, {"table"})
    return render_output(output)


model_tools = robot.with_output_formatter(task_formatter)
```

Pass `model_tools` to the provider adapter. Selection changes model content while
capture and required-camera checks still use the configured cameras. Transforms
keep SI units; rendering converts units once. See [model output](../../docs/model-output.md)
for full output and custom field selection. The example uses the built-in default
formatter, which already summarizes and renders through these same stages.

# Python examples

Start RUA with [your SO-101 connected](../../docs/getting-started.md).
This directory is an independent uv project using the SDK from this checkout.
From the repository root:

```sh
cd examples/python
uv sync --locked
```

For your own agent project, use `uv add "hebbian-rua[openai,anthropic,gemini]"`,
or choose just the provider extras you need. Set up the station with an
[explicit profile](https://github.com/Hebbian-Robotics/rua/blob/v0.1.2/docs/hardware.md)
when using the published 0.1.2 wheel.

## Read your arm's state

From this directory, check that the SDK can see the station and print the
registered arms. This works with torque off and requires no control lease.

```sh
uv run python - <<'PYTHON'
import asyncio
from hebbian_rua import StationClient

async def main():
    async with StationClient() as station:
        state = await station.state()
        for arm in state.arms:
            print(arm.arm, arm.mode, arm.measured_joints)

asyncio.run(main())
PYTHON
```

## Run an agent

The [agent example](agent_quickstart.py) supports OpenAI, Claude and Gemini.

Set `OPENAI_API_KEY`, `ANTHROPIC_API_KEY` or `GEMINI_API_KEY` for your provider.

Set `OPENAI_MODEL`, `ANTHROPIC_MODEL` or `GEMINI_MODEL` to an ID from your account
and pass it with `--model`:

```sh
uv run agent_quickstart.py "Describe the robot state. Keep the arm still." --provider openai --model "$OPENAI_MODEL"
uv run agent_quickstart.py "Describe the robot state. Keep the arm still." --provider anthropic --model "$ANTHROPIC_MODEL"
uv run agent_quickstart.py "Describe the robot state. Keep the arm still." --provider gemini --model "$GEMINI_MODEL"
```

The agent uses the robot model selected by the station. With `--profile auto`,
the station selects the stock or soft-gripper camera model from the arm's
saved setup. The station uses that same model for control, collision checks
and the viewer.

These commands run from this directory. On hardware, verify the arm before
running the agent, release any active manual control lease and supervise the run.
The example enables the arm and gives the model motion tools.
For images, [connect a camera and check its name](../../docs/cameras.md#connect-a-usb-camera),
then add `--camera NAME` and ask the model to describe the view. Repeat the flag
for each camera. With one arm, the example uses its name from station state.
With several arms, pass `--arm NAME` using the name printed by the state check.
The example honors `HEBBIAN_STATION_URL` and `HEBBIAN_MEDIA_SERVER`. Use
`--server` or `--media-server` to override them for a run.

Each run stops after at most 20 model calls by default. Set
`--max-turns` to change that limit. The example uses the provider's async SDK.
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

For a lower-level motion example without a model, see [hello_robot.py](hello_robot.py).
It previews and moves to a fixed target. Before running it on hardware, choose
a target with clearance for your arm and workspace.

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
    output = select_output_cameras(output, {"wrist"})
    return render_output(output)


model_tools = robot.with_output_formatter(task_formatter)
```

Pass `model_tools` to the provider adapter. Selection changes model content while
capture and required-camera checks still use the configured cameras. Transforms
keep SI units; rendering converts units once. See [model output](../../docs/model-output.md)
for full output and custom field selection. The example uses the built-in default
formatter, which already summarizes and renders through these same stages.

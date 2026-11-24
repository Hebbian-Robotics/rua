# Python examples

Start a station with [your arm connected](../../docs/getting-started.md).
For testing without hardware, use a [simulated station](../../docs/simulation.md).
The example environment installs the published Python SDK. From the repository root:

```sh
cd examples/python
uv sync --locked
```

## Read your arm's state

From this directory, run this check with the arm disabled.

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

## Preview and move

[`hello_robot.py`](hello_robot.py) enables the arm, previews a target, moves,
and opens the gripper. Use the [simulation quickstart](../../docs/simulation.md#2-run-hello_robot)
to try it. Check the target and clearance before running on hardware.

## Run an agent

Set `OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, or `GEMINI_API_KEY` in the environment
or `.env`. [`agent_quickstart.py`](agent_quickstart.py) enables the arm and gives
the model motion tools. Pass both the provider and model ID. Release any active
control lease before starting.

```sh
uv run agent_quickstart.py --provider openai --model gpt-6.2-astra \
    "Describe the robot state. Keep the arm still."
```

Run `uv run agent_quickstart.py --help` for arm, camera, station, and model
options. See [Write your own loop](../../docs/write-your-own-loop.md) and
[Model output](../../docs/model-output.md) when adapting the example.

The agent continues each run using OpenAI Responses, Claude Message Threads,
or Gemini Interactions. Each provider holds the conversation history. Subsequent
turns send the new tool results and observations. The Claude SDK can replay
the conversation to restore an expired or interrupted thread.

## Watch live cameras

To watch live cameras alongside the Python agent, use the
[LiveKit room viewer or LiveKit Meet](../../docs/cameras.md#watch-a-usb-camera).

## Inspect a run in Jaeger

[Start Jaeger](https://www.jaegertracing.io/docs/2.23/getting-started/) with the
OTLP HTTP receiver, then run the agent with the trace endpoint:

```sh
OTEL_EXPORTER_OTLP_TRACES_ENDPOINT=http://127.0.0.1:4318/v1/traces \
  uv run agent_quickstart.py --provider openai --model gpt-6.2-astra \
    "Describe the robot state. Keep the arm still."
```

Open [Jaeger](http://127.0.0.1:16686/) and look up the trace ID printed by the
agent, or search for the `hebbian-python-agent` service. Completed model and
tool spans appear as they are exported, so refresh during a longer run. The
top-level run span appears when the run ends. Traces include timing, model and
tool names, and error status. Prompts, images, and tool arguments stay out of
the spans.

Claude request spans come from the Anthropic SDK in metadata-only mode.


## Save the images sent to the model

Set `HEBBIAN_RUA_MODEL_INPUTS_DIR` to save the JPEGs supplied on each model turn:

```sh
HEBBIAN_RUA_MODEL_INPUTS_DIR=/tmp/rua-model-inputs \
  uv run agent_quickstart.py --provider openai --model gpt-6.2-astra \
    --camera wrist "Describe what you see."
```

The agent prints the run directory. Each turn's `index.json` lists the images
in request order with their camera labels and any tool call IDs. With tracing
enabled, the `hebbian.model_inputs.path` span attribute points to that index on
the machine running the agent. Capture covers newly submitted images, including
images returned by tools. Earlier images held by the provider are in their
original turn folders. Choose a persistent path to keep the files beyond
temporary-directory cleanup.

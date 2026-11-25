# Python examples

Start a station with [a real arm connected](../../docs/getting-started.md)
whenever available. Reserve [simulation](../../docs/simulation.md) for testing
without hardware. The examples connect to the station at `HEBBIAN_STATION_URL`
or the local default.
The example environment builds the Python SDK from this checkout. Use a
station built from the same checkout. From the repository root:

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
and opens the gripper. Check the target and clearance before running on hardware.
Release any active control lease before starting. If the viewer holds the
manual lease, follow the [viewer handoff](../../docs/getting-started.md#enable-the-arm-in-agent-mode).
If hardware is unavailable, use the [simulation quickstart](../../docs/simulation.md#2-run-hello_robot).

## Run an agent

Set `OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, or `GEMINI_API_KEY` in the environment
or `.env`. Configure LiveKit on the station for the normal camera workflow.
[`agent_quickstart.py`](agent_quickstart.py) enables the arm and gives the model
motion tools. Pass both the provider and model ID. Release any active control
lease before starting.

```sh
uv run agent_quickstart.py --provider openai --model gpt-6.2-astra \
    --camera wrist --camera overhead \
    "Describe what you see. Keep the arm still."
```

Run `uv run agent_quickstart.py --help` for arm, camera, station, and model
options. See [Write your own loop](../../docs/write-your-own-loop.md) and
[Model output](../../docs/model-output.md) when adapting the example.

The agent continues each run using OpenAI Responses, Claude Message Threads,
or Gemini Interactions. Each provider holds the conversation history. Subsequent
turns send the new tool results and observations. The Claude SDK can replay
the conversation to restore an expired or interrupted thread.

The model can use `update_plan` and `update_notes` to keep track of the task.
Each update replaces the previous plan or notes. The example includes the
current plan and notes in observations for that run.

Plan, notes, preview-only, and sequence-history turns can reuse the last live
observation. The agent refreshes it after motion or a stale observation. If the
station refuses a command because of a stale revision or interlock, the agent
waits for a fresh observation before continuing. A fault or uncertain command
outcome ends the run.

## Watch live cameras

Follow the [camera setup](../../docs/cameras.md#connect-a-usb-camera) to add
named cameras to the station. Repeat `--camera NAME` for each required camera.
Use `--optional-camera NAME` for views the agent can proceed without if they
are unavailable. Use `uv run hebbian-rua camera list` in this example directory
to find the station's camera names.

To add a remote phone camera, configure LiveKit on the station and create an
invite from **Workspace → Camera setup → Remote camera** in the viewer, or run
`uv run hebbian-rua phone-invite overhead` on the station host. Open the invite
on the phone and start streaming. The label `overhead` appears as
`phone_overhead`. Pass `--optional-camera phone_overhead` to the Python example,
or use `--camera phone_overhead` if the run requires it.
See [Add a phone](../../docs/cameras.md#add-a-phone) for setup details.

Follow [watch a USB camera](../../docs/cameras.md#watch-a-usb-camera)
to view the feeds while the agent runs. The camera guide also covers
[options when LiveKit Cloud is unavailable](../../docs/cameras.md#when-livekit-cloud-is-unavailable).

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

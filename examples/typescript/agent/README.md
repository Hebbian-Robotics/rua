# TypeScript reference agent

This example uses the TypeScript client to run a model-driven robot loop. It
supports Anthropic, OpenAI and Gemini, and exposes `runAgent` for the
[viewer agent server](../viewer/README.md) as well as a command-line entry point.

## Run the agent

Follow the [hardware quickstart](../../../docs/getting-started.md#3-verify-manual-control)
to start the station and enable your SO-101. Return any manual lease in the viewer.
From the repository root, set `ANTHROPIC_API_KEY` in the environment or a `.env`
file, then run:

```sh
pnpm install --frozen-lockfile
pnpm --filter @hebbian/agent start --provider anthropic -- arm "Describe the robot state. Keep the arm still."
```

`arm` is the name assigned by native SO-101 setup. For a custom profile, use
the name reported by the station's [state endpoint](http://127.0.0.1:8792/state).

The agent uses the robot model selected by the station. With `--profile auto`,
the station selects the stock or soft-gripper camera model from the arm's
saved setup. The station uses that same model for control, collision checks
and the viewer.

The command loads the local client source directly; it needs no build step.
Rerun it after editing source. For the viewer agent server, `pnpm dev:agent`
watches source changes and `pnpm start:agent` runs until you stop it. See the
[rebuild guide](../../../docs/install-from-source.md#updating-a-checkout).

Select a provider with `--provider anthropic`, `--provider openai` or
`--provider gemini` and set its API key. See the [provider definitions](src/providers/types.ts)
or the viewer's model picker for model defaults. To give the model camera
images, [connect a camera and check its name](../../../docs/cameras.md#connect-a-usb-camera),
then pass `--camera NAME`. Omit the flag for a state-only run.
For a simulated station, see the [MuJoCo example](../../../docs/simulation.md).
Run the command with
`--help` for station, model and other options.
Viewer and CLI runs have no default model-turn limit. Use `--max-turns` to cap
the number of model calls for a CLI run.

See [write your own loop](../../../docs/write-your-own-loop.md#the-typescript-reference-loop)
for the agent's capabilities and programmatic interface, or the
[examples index](../../README.md) for other starting points.

## Tool and output composition

The agent works with its existing tool selection and default output formatter.
The following customization is optional: use it when a task needs different tools
or camera images. You can keep the existing agent loop.

The reference loop uses `mapTools(filterSequenceActions(...))` to expose Cartesian
movement and gripper actions. Each mapping sees the previous result; tool order
and execution restrictions follow the selected capabilities. The SDK's default
formatter uses the shared summarize-then-render pipeline.

For a custom model view, compose transforms independently of capability selection:

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

Use `taskView` for observations and execution. Field selection preserves raw SI
units until rendering; camera selection changes model images without changing
capture or required-camera checks. See [model output](../../../docs/model-output.md).

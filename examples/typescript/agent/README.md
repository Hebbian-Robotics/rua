# TypeScript reference agent

Run a model-driven robot loop from the command line. The example uses the
[TypeScript SDK](../../../typescript-client/README.md) and supports Anthropic,
OpenAI and Gemini.

## Run the agent

Use Node.js 24 and the pnpm version pinned in the root `package.json`. Follow
the [MuJoCo quickstart](../../../docs/simulation.md) or the
[hardware setup guide](../../../docs/getting-started.md) to start the station.
Confirm the arm appears in [station state](http://127.0.0.1:8792/state).
Enable it with the [station CLI](../../../cli/README.md) before starting
the agent:

```sh
cargo run --locked -p hebbian-robot-use-cli -- enable arm
```

Set `ANTHROPIC_API_KEY` in the environment or a local `.env` file. From the
checkout root, try a state-only task:

```sh
pnpm install --frozen-lockfile
pnpm --filter @hebbian/agent start --provider anthropic --max-turns 3 -- arm "Describe the robot state. Keep the arm still."
```

Replace `arm` with the name reported by your station. For OpenAI or Gemini,
select `--provider openai` or `--provider gemini` and set
`OPENAI_API_KEY` or `GEMINI_API_KEY`. Use `--model` to select a model available
to your account. The command loads the local SDK source directly.

Pass `--camera NAME` to attach images from a [configured camera](../../../docs/cameras.md)
or the [MuJoCo snapshot service](../../../docs/simulation.md#camera-snapshots).
Pass `--record` after [enabling recording](../../../docs/recording.md#enable-recording).
Run `pnpm --filter @hebbian/agent start --help` for station, model and other
options. Use `--max-turns` to limit the number of model calls.

## Use the reference loop as a library

`runAgent(config, { signal, onEvent })` runs the same loop as the CLI. It returns
a completed, stopped, incomplete or failed outcome. The loop captures fresh
state and images after physical actions. A current-state `observe` can supply
that capture. History reads and plan or notes updates use the existing
observation. See [`loop.ts`](src/loop.ts) for options and events, or the
[SDK loop guide](../../../docs/write-your-own-loop.md) to build a different
application.

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

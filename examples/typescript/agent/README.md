# TypeScript reference agent

Run an example robot-use agent built with the
[TypeScript SDK](../../../typescript-client/README.md). Give it a task and
the LLM uses robot observations and tools to decide what to do.
The example uses the published TypeScript SDK pinned in the repository lockfile
and supports Anthropic, OpenAI, and Gemini.

## Run the agent

Start a station with [a real arm connected](../../../docs/getting-started.md)
whenever available. Reserve [MuJoCo simulation](../../../docs/simulation.md) for
testing without hardware. Use the Node.js version in the repository's `.nvmrc`
and the pnpm version in `package.json`.

Set `OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, or `GEMINI_API_KEY` in the environment
or `.env`. [Enable the arm in the viewer](../../../docs/getting-started.md#2-verify-manual-control),
then try a state-only task from the repository root.

```sh
pnpm install --frozen-lockfile
pnpm --filter @hebbian/agent start --provider openai --model gpt-6.2-astra \
  --max-turns 3 -- "Describe the robot state. Keep the arm still."
```

Run `pnpm --filter @hebbian/agent start --help` for arm, station, provider,
model, camera, and recording options.

The example uses OpenTelemetry for model and tool spans. It sends span updates
to the station as work starts and finishes. The viewer shows them live, and
station recording saves them for playback.

The Anthropic SDK adds metadata-only request spans under Claude model turns.

## Use the loop in your own code

[`src/loop.ts`](src/loop.ts) exports `runAgent`. See
[Write your own loop](../../../docs/write-your-own-loop.md) for SDK patterns or
[Model output](../../../docs/model-output.md) to customize what the model sees.


## Save the images sent to the model

Set `HEBBIAN_RUA_MODEL_INPUTS_DIR=/tmp/rua-model-inputs` when launching the CLI
or viewer backend. The agent saves JPEGs and an `index.json` for each model
turn with images. The model-turn span links to the index through
`hebbian.model_inputs.path`. See the [file layout and retention notes](../../python/README.md#save-the-images-sent-to-the-model).

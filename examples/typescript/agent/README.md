# TypeScript reference agent

Run a model-driven robot loop using the [TypeScript SDK](../../../typescript-client/README.md).
The example supports Anthropic, OpenAI, and Gemini.

## Run the agent

Start a [simulated station](../../../docs/simulation.md) or
[physical station](../../../docs/getting-started.md). Use the Node.js version
in the repository's `.nvmrc` and the pnpm version in `package.json`.

Set `OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, or `GEMINI_API_KEY` in the environment
or `.env`. From the repository root, enable the arm and try a state-only task.

```sh
cargo run --locked -p hebbian-robot-use-cli -- enable arm
pnpm install --frozen-lockfile
pnpm --filter @hebbian/agent start --max-turns 3 -- "Describe the robot state. Keep the arm still."
```

Run `pnpm --filter @hebbian/agent start --help` for arm, station, provider,
model, camera, and recording options.

## Use the loop in your own code

[`src/loop.ts`](src/loop.ts) exports `runAgent`. See
[Write your own loop](../../../docs/write-your-own-loop.md) for SDK patterns or
[Model output](../../../docs/model-output.md) to customize what the model sees.

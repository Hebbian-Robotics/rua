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

Choose a model ID available in your account. For example,
set `OPENAI_MODEL`, `ANTHROPIC_MODEL` or `GEMINI_MODEL` to that ID, then run:

```sh
uv run agent_quickstart.py "Wave hello" --provider openai --model "$OPENAI_MODEL" --camera table
uv run agent_quickstart.py "Wave hello" --provider anthropic --model "$ANTHROPIC_MODEL" --camera table
uv run agent_quickstart.py "Wave hello" --provider gemini --model "$GEMINI_MODEL" --camera table
```

These commands run from this directory. Use `--help` for the arm, station and
media endpoints. The example uses the provider's official async SDK directly.
Adapters supply `.tools`, a model-ready `.observe()` message, and
`.execute(response)`, which returns `.messages`, `.results` and `.should_stop`.
Append the messages without rebuilding them: they preserve reasoning and
signatures. Stop on `.should_stop` or when `.results` is empty.

Adapters accept complete, non-streaming responses and handle the robot function
calls in that response. Truncated, blocked or otherwise incomplete responses
raise an error before execution. Gemini expects one candidate and manual
function calling; the example disables automatic function calling explicitly.
For mixed toolsets, use `RobotTools` and the `RobotToolCall`/`RobotToolResult`
types to route calls yourself. The adapters don't run a model, own conversation
history or enable the arm.

See [camera setup](../../docs/cameras.md) for feeds and the web preview.
For lower-level SDK calls without a model, see [hello_robot.py](hello_robot.py).

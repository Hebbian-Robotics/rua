# Python SDK

Robot observations, context, tools and execution for your own agents.
Import `RobotTools` and `StationClient` from `hebbian_rua`; use
`OpenAIRobotTools`, `AnthropicRobotTools` or `GeminiRobotTools` from the matching
`hebbian_rua.client.llm.openai`, `.anthropic` or `.gemini` module with your model SDK.
Start with the [read-only state check](../../examples/python/README.md#read-your-arms-state)
or run one of the [agent examples](../../examples/python/README.md#run-an-agent).
`StationClient()` uses `HEBBIAN_STATION_URL` or `http://127.0.0.1:8792` for
station and camera requests. Set `media_server` or `HEBBIAN_MEDIA_SERVER`
to use the media companion directly.
They already handle observations and tool execution with the default model view;
you do not need custom formatters or your own loop.

Use [the control model](../../docs/control-model.md) for a custom control
program, or [model output](../../docs/model-output.md) to change presentation
while keeping the existing loop.

Custom integrations can use `RobotToolCall`, `RobotToolResult`, `RobotToolSpec`
and `RobotObservation` from `hebbian_rua` directly. The core SDK does not import
or require a model provider.

Advanced clients can compile action sequences before acquiring control with
`StationClient.compile_action_sequence(source)`, inspect retained artifacts with
`inspect_compilation(id)`, and submit them with `Lease.start_compiled_sequence(id)`.
Compilation returns a `ProgramCompiled` or `ProgramRejected` value.
Save the inspection artifact's JSON for export and load it with
`import_compilation(artifact)`, which recompiles its source. An optional
`continuation=ProgramContinuation(...)` selects a prior execution's unattempted
suffix explicitly. See the
[compile-before-running example](../../docs/control-model.md#compile-before-running)
for diagnostics, artifact lifetime and execution semantics.

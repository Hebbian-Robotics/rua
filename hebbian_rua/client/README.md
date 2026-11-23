# Python SDK

Robot observations, context, tools and execution for your own agents.
Import `RobotTools` and `StationClient` from `hebbian_rua`; use
`OpenAIRobotTools`, `AnthropicRobotTools` or `GeminiRobotTools` from the matching
`hebbian_rua.client.llm.openai`, `.anthropic` or `.gemini` module with your model SDK.
Start with the [read-only state check](../../examples/python/README.md#read-your-arms-state)
or run one of the [agent examples](../../examples/python/README.md#run-an-agent).
`StationClient()` uses `HEBBIAN_STATION_URL` or `http://127.0.0.1:8792` for
station requests, including observations. Set `media_server` or
`HEBBIAN_MEDIA_SERVER` to use the media companion directly for individual camera
reads and sequence images. The examples use the default model view.

The Python package contains a PyO3 extension for CPython 3.12 and later.
Published wheels include the Rust client. Source installs build it with maturin.
A lease context awaits release on exit. The lease object exposes its status and
operations. Use `Lease.send` and `Lease.cancel_operation` when you need a
caller-assigned operation ID. Use `StationClient.wait_for_command` with a
returned command ID to observe its outcome. A plain sequence wait can be cancelled independently
of the accepted sequence; use `StationClient.cancel_action_sequence` to stop it.
HTTP failures surface as `hebbian_rua.HttpError` with the status and response
body in `status` and `body`.

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

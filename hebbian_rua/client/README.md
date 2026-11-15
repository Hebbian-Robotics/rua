# Python SDK

Robot observations, context, tools and execution for your own agents.
Import `RobotTools` and `StationClient` from `hebbian_rua`; use
`OpenAIRobotTools`, `AnthropicRobotTools` or `GeminiRobotTools` from the matching
`hebbian_rua.openai`, `.anthropic` or `.gemini` module with your model SDK.
See the [agent examples](../../examples/python/README.md).

Custom integrations can use `RobotToolCall`, `RobotToolResult`, `RobotToolSpec`
and `RobotObservation` from `hebbian_rua` directly. The core SDK does not import
or require a model provider.

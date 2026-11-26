# Documentation

Build robot-use agents that let an LLM observe a robot, act through tools and
adapt to the results. Start with a physical arm, then use the SDK to build
your own agent loop.

## Get started

- [Get started with a physical arm](getting-started.md) and run your first agent
  in the browser viewer.
- [Install from source](install-from-source.md): prerequisites and supported
  platforms for the current checkout.
- [Windows with WSL2](windows-wsl.md): RUA requirements and USB setup considerations.
- [SO-101 calibration tool](so101-calibration.md): identify, calibrate and save
  each arm with live 3D feedback.
- [Use an existing LeRobot calibration](hardware.md): configure an explicit
  profile with your arm's calibration ID.

## Build an agent

Start with an example agent. The SDK supplies observations and robot tools.
The station compiles action sequences, executes them under a lease and
reports what happened for the model’s next turn.

- [Run the Python agent](../examples/python/README.md#run-an-agent) with OpenAI,
  Anthropic or Gemini, or use the [TypeScript agent](../examples/typescript/agent/README.md).

- [Write your own loop](write-your-own-loop.md): use the client with your own
  input, planner or model.
- Client libraries: [Python](../hebbian_rua/client/README.md),
  [TypeScript](../typescript-client/README.md), and [Rust](../sdk/README.md).
  Connect to a station and use its typed API and model tools.
- [Model output](model-output.md): defaults, raw results and custom presentation.
- [Examples](../examples/README.md): runnable client, agent and simulator examples.

## Add capabilities

- [Connect cameras](cameras.md): use USB or phone cameras
  with LiveKit and run multiple stations.
- [Record](recording.md): install FFmpeg, start a recording and play
  back saved recordings.
- [Simulation](simulation.md) and [simulator integration](simulators.md).

## Understand and maintain RUA

- [Control model](control-model.md): leases, outcomes, action-sequence compilation,
  artifact export/import and explicit continuation.
- [Architecture](architecture.md): execution flow, components and dependency boundaries.
- [Motor controller](controller.md): the control loop, trajectories, safety
  guards, cycle traces and learned control.
- [Station reference](reference.md): commands, settings and operating behavior.
- [Standalone SO-101 setup package](https://github.com/Hebbian-Robotics/so101/tree/main/setup): calibration
  engine, browser UI, native setup format and its RUA adapter boundary.
- [Control invariants](control-invariants.md): motion admission, geometry guards and failure handling.
- [Contributing](../CONTRIBUTING.md): contributor workflow, validation and style.
- [Development reference](development.md) covers test selection, setup and generated contracts.
- [Release preparation](releasing.md): distribution and checks before publishing.

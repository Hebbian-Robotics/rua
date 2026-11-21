# Documentation

## Get started

- [Get started with a physical arm](getting-started.md): calibrate an SO-101 and
  run your first agent from the viewer.
- [Install from source](install-from-source.md): prerequisites and supported
  platforms for the current checkout.
- [Windows with WSL2](windows-wsl.md): RUA requirements and USB setup considerations.
- [SO-101 calibration tool](so101-calibration.md): identify, calibrate and save
  each arm with live 3D feedback.
- [Use an existing LeRobot calibration](hardware.md): configure an explicit
  profile with your arm's calibration ID.
- [SO-101 manual controls](so101-manual-control.md): select a registered arm
  and control it from the viewer.

## Build an agent

Start with a working example; tool selection and output formatting already have defaults.

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

- [Build another application](extensibility-review.md): APIs and deployment limits
  for custom viewers, planners, camera pipelines and loops.

- [Connect cameras](cameras.md): use USB or phone cameras
  with LiveKit and run multiple stations.
- [Record](recording.md): install FFmpeg, enable recording and play
  back saved recordings.
- [Simulation](simulation.md) and [simulator integration](simulators.md).

## Understand and maintain RUA

- [Control model](control-model.md): leases, outcomes, action-sequence compilation,
  artifact export/import and explicit continuation.
- [Architecture](architecture.md): components, dependency boundaries, and
  [robot support](architecture.md#embodiments-and-controller-support).
- [Viewer development](../examples/typescript/viewer/README.md): developing and building the viewer.
- [Motor controller](controller.md): the control loop, trajectories, safety
  guards, cycle traces and learned control.
- [Station reference](reference.md): commands, settings and operating behavior.
- [Standalone SO-101 setup package](https://github.com/Hebbian-Robotics/so101/tree/main/setup): calibration
  engine, browser UI, native setup format and its RUA adapter boundary.
- [SO-101 self-clipping](so101-self-clipping.md): collision geometry checks.
- [Contributing](../CONTRIBUTING.md): contributor workflow, validation and style.
- [Development reference](development.md): validation details and generated contracts.
- [Testing](../tests/README.md): test selection, ownership and integration boundaries.
- [Release preparation](releasing.md): distribution and checks before publishing.

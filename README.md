# Hebbian Robot-Use Agents (RUA)

Build your own robot-use agents. Give an LLM a goal, camera views
and robot tools, and let it observe, act and adjust.

## Get started

[Set up your SO-101](docs/getting-started.md) to calibrate the arm, start RUA,
verify manual control and run your first agent from the browser.

## Build an agent

RUA supplies robot tools, camera observations and execution. You bring the model
and the loop.

Here's an example agent loop. The [complete example](examples/python/agent_quickstart.py)
includes the connection, lease and arm setup, plus loops for Claude and Gemini:

```python
from hebbian_rua.client.llm.openai import OpenAIRobotTools

robot_tools = OpenAIRobotTools(robot)
messages = [{"role": "user", "content": "Wave hello"}]
for _ in range(20):
    observation = await robot_tools.observe(wait_for_recovery=True)
    messages.append(observation.message)
    response = await client.responses.create(
        model=model,
        instructions=instructions,
        tools=robot_tools.tools,
        input=messages,
    )
    turn = await robot_tools.execute(response, expected_revisions=observation.revisions)
    messages.extend(turn.messages)
    if turn.should_stop or not turn.results:
        break
```

The model chooses tools, RUA executes them, and fresh camera frames and robot
state come back for the next turn. You can change the prompt, model and loop.
Use `AnthropicRobotTools` from `hebbian_rua.client.llm.anthropic` or `GeminiRobotTools` from
`hebbian_rua.client.llm.gemini` with the corresponding SDK. See the
[Python examples](examples/python/README.md).
Choose what reaches the model with [output formatters](docs/model-output.md).

RUA compiles action sequences automatically. The Robot Use Compiler checks and
translates the requested actions. The Robot Use Runtime prepares each step
against live robot state and admits it for execution. Advanced users can
[compile and inspect before running](docs/control-model.md#compile-before-running).
See [how execution works](docs/architecture.md#robot-use-compiler-and-runtime).

See [write your own loop](docs/write-your-own-loop.md) for more on robot tools,
observations and execution.

## In this checkout

- Rust server and media companion
- [Python](hebbian_rua/client/README.md), [TypeScript](typescript-client/README.md) and [Rust](sdk/README.md) SDKs
- [Reference agents](examples/typescript/agent/README.md) and [examples](examples/README.md)
- [CLIs](cli/README.md) for coding agents and shell workflows
- [Browser viewer](examples/typescript/viewer/README.md), manual controls and recording playback
- [LiveKit cameras](docs/cameras.md) and [recordings](docs/recording.md)
- [SO-101 setup](docs/so101-manual-control.md) and [calibration tools](docs/so101-calibration.md)

See the [documentation index](docs/README.md) for the guides, or
[architecture](docs/architecture.md) for component boundaries.

See [SO-101 setup](docs/so101-calibration.md) for hardware and
[LiveKit Cameras](docs/cameras.md#watch-a-usb-camera) to watch live video in your browser.

## Python package

The `hebbian-rua` wheel includes the Python SDK, model adapters, station, media
companion and SO-101 profiles and geometry. Install only the model SDK extras you need.
Prebuilt wheels support Linux x86-64
(glibc 2.39+) and macOS 14+ on Apple Silicon; Windows uses
[WSL2](docs/windows-wsl.md). Add RUA to your agent project
with `uv add hebbian-rua`, or choose an extra such as `uv add 'hebbian-rua[openai]'`.
Run `uv run rua serve --help` for station options. The [hardware profile guide](docs/hardware.md)
shows how to configure a station with an existing calibration.

## Robot support

RUA supports single-arm and bimanual SO-101 setups. It also supports
[learned pose control](docs/hardware.md#learned-policies) on a single arm through
a compatible ONNX policy supplied by the user.

The API can move a real arm. Run it on a trusted network, verify clearance,
supervise initial motions and keep a physical power cutoff accessible. Support
the arm before disabling it. Follow the [hardware operating guide](docs/so101-manual-control.md).

## Research and related projects

Some reading and projects around robot-use agents:

- [Robot-Use Agents](https://web.mit.edu/phillipi/www/writing/robot-use-agents.html) — Phillip Isola's overview of the idea.
- Anthropic's [robotics evaluations](https://www.anthropic.com/research/claude-plays-robotics) and [Fable 5.2 robot-use examples](https://www.anthropic.com/claude-fable-and-mythos-5-2).
- Google's [Gemini 4 Argon for Robotics](https://developers.googleblog.com/gemini-4-argon-for-robotics) — robot control with a general-purpose model.
- NVIDIA's [Astranomy](https://research.nvidia.com/labs/srl/projects/astranomy/) — studying Astra as a robot policy.
- [PEAR](https://mit-han-lab.github.io/pear-blog/) — training reasoning models for robot use with reinforcement learning in simulation.
- [ASPIRE](https://github.com/NVlabs/ASPIRE) — agentic robot skill discovery.
- [ENPIRE](https://github.com/NVlabs/ENPIRE) — robot policy improvement through physical experiments.
- [RAX](https://robot-agent-exploration.com/) — agents learning robot programs through online exploration.
- [RoboColosseum](https://robocolosseum.ai/) — real-world robot evaluation.

## Contributing and license

See [contributing](CONTRIBUTING.md) for contribution workflow, validation and style. The
code is released under the [MIT License](LICENSE); robot models and dependencies
keep their own licenses, listed in [third-party notices](THIRD_PARTY_NOTICES.md).
To report a security problem, see [security](SECURITY.md).

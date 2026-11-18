# Hebbian Robot-Use Agents (RUA)

Build your own robot-use agents. Give an LLM a goal, camera views
and robot tools, and let it observe, act and adjust. We're open sourcing the
parts of our stack that make that loop easy to build. More coming soon.

## Included

- Rust server
- Python SDK and a small agent loop
- [LiveKit camera preview](docs/cameras.md)

Start a project with [uv](https://docs.astral.sh/uv/):

```sh
uv init my-robot-agent
cd my-robot-agent
uv add "hebbian-rua[openai,anthropic,gemini]"
```

Already have a uv project? Start with `uv add`. Choose just the extras you need.
Create a [calibrated robot profile](docs/hardware.md), then start the station with
`uv run rua serve --profile ./robot.toml`, replacing the path with your file.
Prebuilt wheels support Linux x86-64 (glibc 2.39+) and macOS 14+ on Apple Silicon. On Windows, use [WSL2](docs/windows-wsl.md).

See [getting started](docs/getting-started.md) and the [Python examples](examples/python/README.md) to try it,
and the [example applications](examples/applications/README.md) for example toy projects.

See [connecting a simulator](docs/simulators.md) to try in simulation.
To watch your cameras, follow the [LiveKit web preview setup](docs/cameras.md#watch-a-usb-camera).

## Build an agent

RUA supplies robot tools, camera observations and execution. You bring the model
and the loop.

Here's an example agent loop. The [complete example](examples/python/agent_quickstart.py)
includes the connection, lease and arm setup, plus loops for Claude and Gemini:

```python
from hebbian_rua.client.llm.openai import OpenAIRobotTools

robot_tools = OpenAIRobotTools(robot)
messages = [{"role": "user", "content": "Wave hello"}]
while True:
    messages.append(await robot_tools.observe())
    response = await client.responses.create(
        model=model,
        instructions=instructions,
        tools=robot_tools.tools,
        input=messages,
    )
    turn = await robot_tools.execute(response)
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

Hardware setup, camera tools and recording utilities have their own guides:
[SO-101](https://github.com/Hebbian-Robotics/so101),
[LiveKit Cameras](https://github.com/Hebbian-Robotics/livekit-cameras) and
[MCAP Tools](https://github.com/Hebbian-Robotics/mcap-tools). RUA handles their
integration with your robot and agent.

## Coming soon

- TypeScript and Rust SDKs
- CLIs
- Browser viewer and manual controls
- More agents
- More examples and integrations
- Recording playback and analysis
- Policy and training tools

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

See [contributing](CONTRIBUTING.md). Code is [MIT](LICENSE); models and dependencies
keep their own licenses. See [third-party notices](THIRD_PARTY_NOTICES.md).

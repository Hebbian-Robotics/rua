# Examples

These examples use the RUA client and server to build robot use agents and other
robot applications. Use them as starting points for your own code, in Python or
TypeScript. [Set up your SO-101](../docs/getting-started.md) and run the commands
from the repository root. The Python motion examples enable the arm themselves.
Before starting the TypeScript agent, enable the arm and return any active lease.
Export `HEBBIAN_STATION_URL` for a nondefault station address. The agent
quickstart and the TypeScript agent also accept `--server`.

Examples that request images use the [name you registered for that camera](../docs/cameras.md#connect-a-usb-camera).
Camera calls use the station URL by default. For direct media access, export
`HEBBIAN_MEDIA_SERVER` or pass `--media-server` to the Python quickstart or
TypeScript agent.

## [Python](python/README.md)

| Example | What it is | Needs | Run |
| --- | --- | --- | --- |
| [Read your arm's state](python/README.md#read-your-arms-state) | Read connected arms, modes and joint positions without enabling motion. | A station | [Run the state check](python/README.md#read-your-arms-state) |
| [`hello_robot.py`](python/hello_robot.py) | Enable the arm, preview a target, move, check the [command outcome](../docs/write-your-own-loop.md#commands-and-outcomes), and open the gripper. | A station | `uv run --project examples/python python examples/python/hello_robot.py` |
| [`agent_quickstart.py`](python/agent_quickstart.py) | Agent loops for OpenAI, Anthropic and Gemini using the robot tool adapters. | A station, your provider API key and a model ID | [Run an agent](python/README.md#run-an-agent) |

Each Python example directory has its own `pyproject.toml`, lockfile and uv
environment. Local source overrides use the Python SDK in this checkout.
For complete projects using wrist-camera images, see the
[example applications](applications/README.md).

## TypeScript (`typescript/`)

| Example | What it is | Needs | Run |
| --- | --- | --- | --- |
| [`agent/`](typescript/agent/README.md) | The full reference agent (`@hebbian/agent`): Claude, OpenAI and Gemini through their official SDKs, plan and notes, tracing, token usage. Exports `runAgent`; the CLI is a thin wrapper for the terminal. | A station, a provider API key, `pnpm install` | [Run the reference agent](typescript/agent/README.md#run-the-agent) |

The agent declares its dependencies in `typescript/agent/package.json`.

## Development simulator fixture

| Example | What it is | Needs | Run |
| --- | --- | --- | --- |
| [`mujoco_sim/`](mujoco_sim/README.md) | Minimal MuJoCo fixture for control, multi-arm plant transport and camera validation. Start the station with a [fixture profile](mujoco_sim/profiles/single.toml), then point the fixture at the station work directory. Users implement the [simulator contract](../docs/simulators.md) for their own engine and scene. | `uv sync --project examples/mujoco_sim` | `uv run --project examples/mujoco_sim python examples/mujoco_sim/run.py --socket-directory DIR` |

The [`scripts/`](../scripts/) directory holds hardware diagnostics and
installed-package smoke checks. Run `uv run rua-dev contracts` to regenerate
schemas and SDK contracts. These development tools are not needed to run or write a loop.

To build your own loop with other inputs or another model SDK, read
[write your own loop](../docs/write-your-own-loop.md).

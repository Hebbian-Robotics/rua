# Examples

These examples use the RUA client and server to build robot use agents and other
robot applications. Use them as starting points for your own code, in Python or
TypeScript. [Set up your SO-101](../docs/getting-started.md) and run the commands
from the repository root. The Python motion examples enable the arm themselves.
Before starting the TypeScript agent, enable the arm in the viewer and return
any manual lease.
Export `HEBBIAN_STATION_URL` for a nondefault station address. The agent
quickstart and the TypeScript agent also accept `--server`.

Examples that request images use the [name you registered for that camera](../docs/cameras.md#connect-a-usb-camera).
For nondefault media addresses, export `HEBBIAN_MEDIA_SERVER` or pass
`--media-server` to the Python quickstart or TypeScript agent.

## [Python](python/README.md)

| Example | What it is | Needs | Run |
| --- | --- | --- | --- |
| [Read your arm's state](python/README.md#read-your-arms-state) | Read connected arms, modes and joint positions without enabling motion. | A station | [Run the state check](python/README.md#read-your-arms-state) |
| [`hello_robot.py`](python/hello_robot.py) | Enable the arm, preview a target, move, check the [command outcome](../docs/write-your-own-loop.md#commands-and-outcomes), and open the gripper. | A station | `uv run --project examples/python python examples/python/hello_robot.py` |
| [`agent_quickstart.py`](python/agent_quickstart.py) | Agent loops for OpenAI, Anthropic and Gemini using the robot tool adapters. | A station, your provider API key and a model ID | [Run an agent](python/README.md#run-an-agent) |

Each Python example directory has its own `pyproject.toml`, lockfile and uv
environment. Local source overrides use the SDK and tools packages in this checkout.
For complete projects using wrist-camera images, see the
[example applications](applications/README.md).

## TypeScript (`typescript/`)

| Example | What it is | Needs | Run |
| --- | --- | --- | --- |
| [`agent/`](typescript/agent/README.md) | The full reference agent (`@hebbian/agent`): Claude, OpenAI and Gemini through their official SDKs, plan and notes, tracing, token usage. Exports `runAgent`; the CLI is a thin wrapper for the terminal. | A station, a provider API key, `pnpm install` | [Run the reference agent](typescript/agent/README.md#run-the-agent) |
| [`viewer/`](typescript/viewer/README.md) | Browser viewer: live 3D arm, cameras, manual control, recordings, run panel. Its `server/` is the agent server that runs the agent in-process (`runAgent`). | `pnpm install --frozen-lockfile && pnpm build` | Served at <http://127.0.0.1:8792/viewer/>; `pnpm dev` for hot reload; `pnpm test:viewer` for its tests |

The agent declares its dependencies in `typescript/agent/package.json`. The
viewer and its agent server use the root `package.json`; run their commands
from the repository root.

## Development simulator fixture

| Example | What it is | Needs | Run |
| --- | --- | --- | --- |
| [`mujoco_sim/`](mujoco_sim/README.md) | Minimal MuJoCo fixture for control, multi-arm plant transport and camera validation. Start the station with a [fixture profile](mujoco_sim/profiles/single.toml), then point the fixture at the station work directory. Users implement the [simulator contract](../docs/simulators.md) for their own engine and scene. | `uv sync --project examples/mujoco_sim` | `uv run --project examples/mujoco_sim python examples/mujoco_sim/run.py --socket-directory DIR` |

Recording playback without a station: `pnpm build && cargo run -p recordings -- --directory recordings`.

The [`scripts/`](../scripts/) directory holds policy export, hardware diagnostics
and installed-package smoke checks. Schema and contract generation lives in
`uv run rua-dev`; `pnpm generate:schemas` and `pnpm check:station-contracts` invoke
it directly. These development tools are not needed to run or write a loop.

To build your own loop with other inputs or another model SDK, read
[write your own loop](../docs/write-your-own-loop.md).

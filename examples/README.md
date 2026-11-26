# Examples

Start with the [browser viewer quickstart](../docs/getting-started.md) to set up
your arm and see a robot-use agent in action. Use a real arm whenever available.
Reserve [MuJoCo simulation](../docs/simulation.md) for testing without hardware.

| Example | What to try |
| --- | --- |
| [Browser viewer](typescript/viewer/README.md) | See live arm state and cameras, control the arm, run an agent, and review recordings. |
| [Python examples](python/README.md) | Use the Python SDK to read arm state, preview a move, or write an agent loop. |
| [TypeScript agent](typescript/agent/README.md) | Run the reference agent from the terminal or import its loop. |
| [Example applications](applications/README.md) | Inspect blocks or answer questions about an image using the wrist camera. |
| [MuJoCo](mujoco_sim/README.md) | Run a simulated station with one or two SO-101 arms. |

Each example guide describes its scope, including behavior you would need to
add when adapting the example.

See [Write your own loop](../docs/write-your-own-loop.md) and the
[Python](../hebbian_rua/client/README.md), [Rust](../sdk/README.md), and
[TypeScript](../typescript-client/README.md) SDK guides when you’re ready to
build your own agent.

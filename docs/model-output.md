# Choose what the model sees

The Python SDK keeps full station data and supplies a concise model view by
default. Observations include measured joints, tool pose, gripper state, faults,
interlocks and configured camera images. Motion results retain outcomes,
achieved poses and limits. Internal transport IDs and repeated history are
omitted. Model values use millimetres and degrees; raw SDK data remains in SI.

The default robot tools are `action_sequence`, `preview_pose`, `observe` and
`stop`. Tool selection and output formatting are independent. The
[Python example](../examples/python/README.md) filters sequences to Cartesian
movement and gripper actions and adds its own `finish` tool.

## Full output

Starting with your existing `robot`, pass a different view to the provider adapter:

```python
from hebbian_rua import full_formatter
from hebbian_rua.client.llm.openai import OpenAIRobotTools

model_tools = OpenAIRobotTools(robot.with_output_formatter(full_formatter))
```

Full output retains original fields, units and precision. Application code can
always inspect `result.data` independently of the model view.

## Customize presentation

Compose ordinary transforms before rendering:

```python
from hebbian_rua import (
    ModelContent,
    RobotOutput,
    render_output,
    select_output_cameras,
    summarize_output,
)


def task_formatter(output: RobotOutput) -> ModelContent:
    output = summarize_output(output)
    output = select_output_cameras(output, {"wrist"})
    return render_output(output)


model_tools = robot.with_output_formatter(task_formatter)
```

The new view shares the station and lease and leaves the original unchanged.
Transforms preserve SI units; rendering converts once. Camera selection changes
model images without changing capture or required-camera checks. Formatting
cannot change execution flags. Formatter exceptions propagate after execution
and do not retry a robot command.

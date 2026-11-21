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

## Rust SDK and CLI

In Rust, `result.model_data()` gives the concise view and `result.data()` returns
the complete payload in SI units. The CLI prints the concise view by default.
Pass `--raw` for the full payload. See [Rust SDK output formatting](../sdk/README.md#raw-results-and-model-views)
and [CLI output and units](../cli/README.md).

## Images during action sequences

With named cameras configured, the SDK can retain up to eight JPEGs from the
latest model action sequence. Request them with the `observe` input
`{"source": "last_sequence"}`. Without that input, observation returns current
images. Sequence images can include timing gaps and are independent of station recording.

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

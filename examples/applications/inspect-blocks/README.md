# Inspect Blocks

Arrange blocks in front of the arm and ask a yes/no question, such as “Are the
blocks stacked?” The model moves the arm to inspect the scene with the wrist
camera. The arm nods for yes or shakes its head for no.

## Run

[Set up the arm](../../../docs/getting-started.md), start the station with a
[registered wrist camera](../../../docs/cameras.md#connect-a-usb-camera), and set
`OPENAI_API_KEY`. Use a supported resting pose and verify clearance for
`PRESENT_JOINTS` and the gestures in [`inspect_blocks.py`](inspect_blocks.py).
Start with the arm disabled in a supported resting pose and release any active
control lease. If the viewer holds the manual lease, use the
[disable controls](../../../docs/so101-manual-control.md#controls) first.
The application enables the arm and saves that pose for rehoming.

From the repository root, replace `wrist` with the camera's registered name.

```sh
uv run --project examples/python examples/applications/inspect-blocks/inspect_blocks.py --camera wrist
```

Use `--workspace` to describe the area to inspect. The arm rehomes and disables
between questions. Press Enter on an empty line to quit. Open `runs/` next to
the script to review the frames and moves for each answer.

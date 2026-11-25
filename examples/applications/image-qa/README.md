# Image Q&A

Show an image on a display and ask a yes/no question such as “Is this a Siberian
Husky?” The arm moves to a viewing pose, sends a wrist-camera frame to the model
for web research, and nods or shakes its head.

## Run

[Set up the arm](../../../docs/getting-started.md), start the station with a
[registered wrist camera](../../../docs/cameras.md#connect-a-usb-camera), and set
`OPENAI_API_KEY`. Use a supported resting pose and verify clearance for
`VIEW_JOINTS` and the gestures in [`image_qa.py`](image_qa.py).
Release any active control lease before starting. If the viewer holds the
manual lease, follow the [viewer handoff](../../../docs/getting-started.md#enable-the-arm-in-agent-mode).

From the repository root, replace `wrist` with the camera's registered name.

```sh
uv run --project examples/python examples/applications/image-qa/image_qa.py --camera wrist
```

Press Enter on an empty line to rehome, disable, and quit. Open `runs/` next to
the script to review the camera frame and reply. For questions about a person,
include their name in the question or show it on the display.

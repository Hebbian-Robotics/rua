# Get started with an SO-101

Use the browser viewer to verify manual control and run your first agent.
Use the examples from [RUA's main branch](https://github.com/Hebbian-Robotics/rua/tree/main)
with the published station and SDKs. This guide uses 0.1.7. For development,
follow [Install from source](install-from-source.md).

If you need to calibrate your arm, follow the [SO-101 calibration guide](so101-calibration.md).

## 1. Install RUA

Install [uv](https://docs.astral.sh/uv/getting-started/installation/) and
[build the viewer](install-from-source.md#optional-viewer) from a checkout. See
[Windows with WSL2](windows-wsl.md) for Windows setup considerations.

## 2. Verify manual control

Support the arm before disabling it. Verify clearance from objects and other
arms, supervise the first motions, and keep a physical power cutoff accessible.

Before starting the viewer, [configure LiveKit on the station using `lk cloud auth` or `.env`](cameras.md#connect-a-usb-camera). Then run the station from the repository root:

```sh
uvx --from hebbian-rua==0.1.7 rua serve
```

Set at least one model-provider key (`OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, or
`GEMINI_API_KEY`) in your environment or the repository’s `.env`. Then start the
viewer in a second terminal with `pnpm start:viewer`.
Wait for `measured_joints` to populate in [station state](http://127.0.0.1:8792/state).
The API can respond before arm feedback arrives. With torque off, open the
[viewer](http://127.0.0.1:8793/viewer/) and
[check that its 3D view matches the arm](so101-manual-control.md#set-up-each-so-101).
Choose **Manual**, then select **Enable arm** to hold its current pose and
acquire manual control.
Select **Use current position** to preview a goal, and use **Move to goal**
only after checking the preview and physical clearance.
See [viewer controls](so101-manual-control.md#controls) for details and
[fault recovery](so101-manual-control.md#recovering-from-a-fault) if needed.

## 3. Run your first agent

### Enable the arm in Agent mode

If the arm is enabled in **Manual**, disable it first. Use
**Rehome and disable** if the saved pose lets the arm rest securely and the
path is clear.
To remove torque at the current pose, support the arm before confirming
**Disable in place…**. Wait for the arm to show **disabled**. The viewer
releases its manual lease when the last enabled arm is disabled.

Under **Control mode**, select **Agent**, then select **Enable arm** and wait
for the arm to show **holding**. Enabling in Agent mode leaves the lease
available for the program that runs the task.

### Start a task in the viewer

1. In **Run a task**, choose your provider and model. If you set up cameras,
   confirm their live feeds and select the views to share under **Run options**.
2. With the arm enabled and holding, enter “Describe what you can observe. Keep
   the arm and gripper still.” Select **Start task** and check the reply. The
   agent can move the arm, so supervise the run and keep **Stop run** and the
   physical power cutoff accessible.

Use **Attach image** to send one JPEG, PNG, or WebP reference with the prompt.
The viewer shows a thumbnail before the run and saves the filename in the run
record. Select **Remove image** to clear the attachment.

## Next steps

- [Run and customize the Python example](../examples/python/README.md) or use
  the [TypeScript reference agent](../examples/typescript/agent/README.md).
- [Write your own loop](write-your-own-loop.md) using the SDK.
- [Record a session](recording.md).
- Browse the [examples](../examples/README.md) and [documentation index](README.md).

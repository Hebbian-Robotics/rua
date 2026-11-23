# Get started with an SO-101

Use the browser viewer to verify manual control and run your first agent.
These steps use the source checkout.

If you need to calibrate your arm, follow the [SO-101 calibration guide](so101-calibration.md).

## 1. Install RUA

Complete the [source setup](install-from-source.md#prepare-the-checkout) and
[build the viewer](install-from-source.md#optional-viewer). See
[Windows with WSL2](windows-wsl.md) for Windows setup considerations.

## 2. Verify manual control

Support the arm before disabling it. Verify clearance from objects and other
arms, supervise the first motions, and keep a physical power cutoff accessible.

Before starting the viewer, [add your LiveKit credentials to the station’s `.env`](cameras.md#connect-a-usb-camera). Then run the station from the repository root:

```sh
uv run cargo run -p hebbian-robot-use-server -- serve --manual-control
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

1. In **Run a task**, open **Agent settings** and choose your provider. If you
   set up cameras, confirm their live feeds and select the views to share.
2. With the arm enabled and holding, enter “Describe what you can observe. Keep
   the arm and gripper still.” Select **Start task** and check the reply. The
   agent can move the arm, so supervise the run and keep **Stop run** and the
   physical power cutoff accessible.

## Next steps

- [Run and customize the Python example](../examples/python/README.md) or use
  the [TypeScript reference agent](../examples/typescript/agent/README.md).
- [Write your own loop](write-your-own-loop.md) using the SDK.
- [Record a session](recording.md).
- Browse the [examples](../examples/README.md) and [documentation index](README.md).

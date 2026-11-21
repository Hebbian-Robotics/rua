# Get started with an SO-101

Calibrate your SO-101, verify manual control and run your first agent in the
viewer. These steps use the source checkout. If you're using the published
0.1.2 wheel, follow the [0.1.2 hardware guide](https://github.com/Hebbian-Robotics/rua/blob/v0.1.2/docs/hardware.md)
to create a profile with LeRobot calibration.

## 1. Install RUA

Complete the [source setup](install-from-source.md#prepare-the-checkout) and
[build the viewer](install-from-source.md#optional-viewer), then return here to
calibrate your arm. See [Windows with WSL2](windows-wsl.md) for Windows setup
considerations.

## 2. Calibrate your arm

Follow the [SO-101 calibration guide](so101-calibration.md) to run
`so101 setup` and save your arm's `setup.toml` and `calibration.json`.
The station uses these files to find and configure your arm by default.
It exposes the arm as `arm` in the SDKs and command-line tools. For a custom
profile, use the name reported in [station state](http://127.0.0.1:8792/state).
If you've already saved both files for this arm, continue to station startup.

If you've already calibrated the arm with LeRobot, you can
[reuse that calibration with an explicit profile](hardware.md).

Support the arm before disabling it. Verify clearance from objects and other
arms, supervise the first motions, and keep a physical power cutoff accessible.

## 3. Verify manual control

To stream camera video to the viewer, follow the [LiveKit camera setup](cameras.md#connect-a-usb-camera)
before starting. Then run the station from the repository root:

```sh
uv run cargo run -p hebbian-robot-use-server -- serve --manual-control
```

Wait for `measured_joints` to populate in [station state](http://127.0.0.1:8792/state).
The API can respond before arm feedback arrives. With torque off, open the
[viewer](http://127.0.0.1:8792/viewer/) and
[check that its 3D view matches the arm](so101-manual-control.md#set-up-each-so-101).
Choose **Manual**, then select **Enable arm** to hold its current pose and
acquire manual control.
Select **Target current pose** to preview a target, and use **Move to target**
only after checking the preview and physical clearance.
See [viewer controls](so101-manual-control.md#controls) for details and
[fault recovery](so101-manual-control.md#recovering-from-a-fault) if needed.

## 4. Run your first agent

1. Set `OPENAI_API_KEY`, `ANTHROPIC_API_KEY` or `GEMINI_API_KEY` in your
   environment or the repository's `.env`. Keep the station running and start
   the agent server from the repository root.

   ```sh
   pnpm start:agent
   ```

2. In **Run a task**, open **Agent settings** and choose your provider. If you
   set up cameras, confirm their live feeds and select the views to share.
3. With the arm enabled and holding, enter “Describe what you can observe. Keep
   the arm and gripper still.” Select **Start task** and check the reply. The
   agent can move the arm, so supervise the run and keep **Stop run** and the
   physical power cutoff accessible.

## Next steps

- [Run and customize the Python example](../examples/python/README.md) or use
  the [TypeScript reference agent](../examples/typescript/agent/README.md).
- [Write your own loop](write-your-own-loop.md) using the SDK.
- [Record a session](recording.md).
- Browse the [examples](../examples/README.md) and [documentation index](README.md).

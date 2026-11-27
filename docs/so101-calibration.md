# SO-101 calibration with RUA

Use the `so101` setup CLI from [Hebbian-Robotics/so101](https://github.com/Hebbian-Robotics/so101)
to set up a physical SO-101 arm for RUA. It saves `setup.toml` and
`calibration.json` together under `arms/<USB-serial>/` and writes the standard
servo registers. RUA uses both files when `serve` automatically selects the arm.

With the arm supported, torque off and the station stopped, run the SO-101
CLI included in the RUA release wheel from your uv project. This command is available in the release after RUA 0.1.7:

```sh
uv run rua so101 setup
```

Outside a uv project, use `uvx --from hebbian-rua rua so101 setup` with a release that includes the SO-101 CLI.

For source checkouts, download the `so101` archive from
[GitHub Releases](https://github.com/Hebbian-Robotics/so101/releases), put it on
your PATH, and run `so101 setup`. RUA 0.1.7 and older releases use a different
arm configuration directory; consult the documentation for that release.

New arms default to the soft gripper with wrist camera; add
`--model-variant stock` for stock gripper geometry. Existing entries retain
their model. Open the local URL printed by the tool and choose **Calibrate arm**.
Select your arm, record its ranges and review the live model. Move shoulder pan
and the gripper separately by hand with torque off to confirm each moves the
matching joint in the preview, then save. Saving backs up previous files
and registers, verifies settings with torque off, and releases the port.
Reconcile the reported backup if save fails. See the
[SO-101 setup project](https://github.com/Hebbian-Robotics/so101/tree/main/setup)
for the calibration controls.

Return to [Getting started](getting-started.md) to start the station and verify
the physical arm against the model before powered operation.

## Motor mapping

The tool starts with IDs 1 through 6 from base to gripper. If your arm uses
another order, open **Edit motor mapping** during the full sweep. Use the ascending
or descending shortcut, or assign an ID to each joint. The save writes
`[motor_ids]` to `setup.toml`, and RUA uses it to load the matching calibration.
Recorded travel follows each physical motor when the mapping changes.

## Custom setup directory

If you keep arm configs in another directory, set
`SO101_SETUP_DIRECTORY` to its path in the environment for the setup tool
and the station. Keep both files together when backing up or sharing an arm's
setup with your team.

To reuse a calibration you already created with LeRobot, see
[Use an existing LeRobot calibration](hardware.md).

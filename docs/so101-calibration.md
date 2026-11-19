# SO-101 calibration with RUA

Use `so101-calibrate` to set up a physical SO-101 follower for RUA. The tool saves
`setup.toml` and `calibration.json` together under `arms/<USB-serial>/`. RUA uses
both files when you start the station with `--profile auto`.

Install the calibration tool version supported by this checkout. The command
uses Python 3.12 and a `rustypot` pin to install from prebuilt wheels.

```sh
uv tool install --python 3.12 --with 'rustypot==1.13.1' \
  'https://github.com/Hebbian-Robotics/so101/releases/download/v0.1.5/hebbian_so101_setup-0.1.5-py3-none-any.whl?download=1'
```

With the arm supported, torque off and the station stopped, run the tool using
RUA's default arm configuration directory. This workflow uses motor IDs
`1, 2, 3, 4, 5, 6` from base to gripper:

```sh
so101-calibrate --setup-directory "${XDG_CONFIG_HOME:-$HOME/.config}/rua/so101-arm-configs"
```

For a new arm with the soft gripper and wrist camera, add
`--model-variant soft-gripper-camera`. Existing entries retain their model and
servo settings. Open the local URL printed by the tool and choose **Calibrate arm**.
Select your arm, record its ranges and review the live model. Move shoulder pan
and the gripper separately by hand with torque off to confirm each moves the
matching joint in the preview, then save. Saving backs up previous files
and registers, verifies settings with torque off, and releases the port.
Reconcile the reported backup if save fails. See the
[SO-101 setup project](https://github.com/Hebbian-Robotics/so101/tree/v0.1.5/setup)
for the calibration controls.

Return to [Getting started](getting-started.md#set-up-a-physical-so-101) to start the station and verify
the physical arm against the model before powered operation.

## Custom setup directory

If you keep arm configs in a private repository or another local directory,
pass its path to `so101-calibrate --setup-directory` and to RUA's
`--so101-config-directory`, or set `HEBBIAN_SO101_CONFIG_DIRECTORY` for RUA.
Keep both files together when backing up or sharing an arm's setup with your team.

To reuse a calibration you already created with LeRobot, see
[Use an existing LeRobot calibration](hardware.md).

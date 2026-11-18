# Set up each SO-101

[SO-101](https://github.com/Hebbian-Robotics/so101) owns the driver, calibration
tool and model assets. Its [setup guide](https://github.com/Hebbian-Robotics/so101/tree/main/setup)
covers calibration; the steps below connect a calibrated arm to RUA profiles.
RUA reads LeRobot calibration JSON; the standalone tool's `setup.toml` is not a
RUA profile.

Every physical arm needs its own serial port, LeRobot calibration and verified
model mapping. The bundled profile provides starting values, not a calibration
or universal servo tune. Keep the arm supported with torque off during setup,
and keep a physical power cutoff accessible.

1. Follow [LeRobot's SO-101 setup](https://huggingface.co/docs/lerobot/en/so101)
   to identify the port and calibrate with a unique robot ID. The station reads
   `~/.cache/huggingface/lerobot/calibration/robots/so_follower/<id>.json`.
   Prefer a stable `/dev/serial/by-id/` path on Linux; on macOS, use the arm's
   `/dev/cu.*` serial device.
2. Create a private profile such as `$HOME/.config/rua/so101.toml`:

   ```toml
   extends = "so101-soft-gripper-camera"
   name = "my-so101"

   [feetech]
   ports = { follower = "/dev/serial/by-id/your-arm" }
   calibration_ids = { follower = "your-calibration-id" }
   joint_signs = [1, 1, 1, 1, 1]
   joint_offsets_radians = [0.0, 0.0, 0.0, 0.0, 0.0]
   ```

   This default models the original-scale TPU gripper, camera bracket and
   HBVCAM-W20217 V22 wrist camera supplied with our setup. For the stock rigid
   gripper without that camera assembly, use `extends = "so101"`.

3. With the station stopped and torque off, apply and verify the profile's expected
   servo gains using your hardware setup tools. Station startup verifies the gains
   but does not write them. Recheck them after calibration.
4. From a source checkout, start the current station code:

   ```sh
   uv run cargo run -p hebbian-station -- serve --profile "$HOME/.config/rua/so101.toml"
   ```

   With an installed wheel, use `uv run rua serve` with the same profile argument.
   Before enabling, move each joint gently with
   torque off and compare SDK measured joints and tool poses to the physical
   arm and bundled model. Verify signs, offsets, calibrated endpoints and gripper
   travel on this assembly. Check the configured model in the viewer before motion.

Profile and calibration changes take effect after a station restart. Support
and disable the arm before restarting. Calibrated endpoints define joint limits;
the URDF supplies geometry. Neither establishes physical clearance or support.
Back up profiles and calibration privately; never include them in the public repo.

Before increasing speed, assess target versus measured joint angles, command
outcomes, effort/load, temperature, voltage and clearance. Compare actual PID
registers with the per-arm profile. Do not copy another arm's gains or relax
limits to conceal poor tracking. The controller is soft real time and its
collision checks only cover configured geometry, not unknown workspace objects.

To estimate following lag from executor cycle traces, use completed runs:

```sh
uv run python scripts/estimate_following_lag.py diagnostics/executor-*-trace.jsonl
```

For each joint, the script reports `following_lag_seconds` from the slope of
tracking error against commanded velocity during steady-speed cruise. The error
compares each measured position with the command sent in the preceding cycle;
the intercept captures the load's static offset. Lag should remain roughly
constant across speed bands; growth in the highest band can indicate saturation.
The script also reports a cross-correlation delay and peak speed measured over
100 ms. Cruise samples before a tripped move remain valid.

# Use an existing LeRobot calibration

If you already calibrated your SO-101 with LeRobot, point a local RUA profile
at its calibration ID and serial port. For a new arm, follow the
[SO-101 calibration guide](so101-calibration.md).

The profile below uses the current source tree. For the published 0.1.2 wheel,
use the [0.1.2 profile format](https://github.com/Hebbian-Robotics/rua/blob/v0.1.2/docs/hardware.md).

Every physical arm needs its own serial port, LeRobot calibration and verified
model mapping. The bundled profile provides starting values, not a calibration
or universal servo tune. Keep the arm supported with torque off during setup,
and keep a physical power cutoff accessible.

1. Find the robot ID you used in [LeRobot's SO-101 setup](https://huggingface.co/docs/lerobot/en/so101).
   By default, the station reads
   `~/.cache/huggingface/lerobot/calibration/robots/so_follower/<id>.json`
   using that ID. Identify the arm's serial port. Prefer a stable
   `/dev/serial/by-id/` path on Linux. On macOS, use the arm's `/dev/cu.*` device.
2. Create a private profile such as `$HOME/.config/rua/so101.toml`:

   ```toml
   extends = ["so101", "so101-soft-gripper-camera-assembly"]
   name = "my-so101"
   arms = ["arm"]

   [feetech]
   ports = { arm = "/dev/serial/by-id/your-arm" }
   calibration_ids = { arm = "your-calibration-id" }
   joint_signs = [1, 1, 1, 1, 1]
   joint_offsets_radians = [0.0, 0.0, 0.0, 0.0, 0.0]
   ```

   The key `arm` is the station name you will see in state and use in clients.
   Use the same name in `arms`, `ports` and `calibration_ids`.
   This example models the original-scale TPU gripper, camera bracket and
   HBVCAM-W20217 V22 wrist camera supplied with our setup. For the stock rigid
   gripper without that camera assembly, use `extends = "so101"`.

3. With the station stopped and torque off, apply and verify the servo gains
   using your hardware setup tools. The pinned so101 runtime defines the gains
   by joint, not the station profile. Station startup verifies the gains
   but does not write them. Recheck them after calibration.
4. From a source checkout, start the current station code:

   ```sh
   uv run cargo run -p hebbian-robot-use-server -- serve --profile "$HOME/.config/rua/so101.toml"
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
registers with the gains the so101 runtime defines for each joint. Do not copy another arm's gains or relax
limits to conceal poor tracking. The controller is soft real time and its
collision checks only cover configured geometry, not unknown workspace objects.

To estimate following lag, run the trace analyzer on a completed run's JSONL
file. Single-arm stations write `controller-*-trace.jsonl` files under
`traces/controller/` by default. Substitute the path to the completed run below.
If you set `--trace-directory`, look there. Multi-arm runs use per-arm
subdirectories.

```sh
uv run --with numpy python scripts/estimate_following_lag.py "$TRACE_PATH"
```

For each joint, the script reports `following_lag_seconds` from the slope of
tracking error against commanded velocity during steady-speed cruise. The error
compares each measured position with the command sent in the preceding cycle;
the intercept captures the load's static offset. Lag should remain roughly
constant across speed bands; growth in the highest band can indicate saturation.
The script also reports a cross-correlation delay and peak speed measured over
100 ms. Cruise samples before a tripped move remain valid.

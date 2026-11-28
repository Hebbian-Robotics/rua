# Hardware

## Supported hardware

RUA supports single-arm and bimanual SO-101 setups on hardware and in
[MuJoCo](simulation.md). Both support planned IK and joint motion. In a
bimanual profile, check each `base_poses` transform against the physical mount
before motion.

### Learned policies

For a single SO-101 arm, set `controller = "policy"` in the robot profile to
run pose goals through the ONNX policy adapter. Supply a compatible model for
the configured arm geometry, set `[policy].onnx` to its path, and make ONNX Runtime available on the station
host. The adapter checks the model's observation, action and geometry contract.
Use the [policy settings profile](../profiles/so101-policy-settings.toml) as a
starting point and check the policy's behavior on your setup before powered
operation.

The station loads exported ONNX models. A model takes `goal` (1×21) and
`history` (1×150) float inputs, returns `action` (1×5), and carries these
metadata entries:

| Key | Meaning |
|---|---|
| `method` | `supermimic_official_so101_pose_v1`, the observation contract |
| `action_mode` | `default_pose_offset`: target = offset + scale × action |
| `action_offset_radians`, `action_scale_radians` | JSON lists of 5: the target's offset and positive scale |
| `action_step` | Furthest a target may lead the measured joint, in radians |
| `observation_joint_offset` | JSON list of 5 joint offsets subtracted from observations |
| `hardware`, `station_urdf_sha256` | Optional binding to one URDF; other arms are refused |

## Use an existing LeRobot calibration

If you already calibrated your SO-101 with LeRobot, point a local RUA profile
at its calibration ID and serial port. For a new arm, follow the
[SO-101 calibration guide](so101-calibration.md).

The profile below works with RUA 0.1.9. Use the hardware guide from the matching
checkout if you are running an earlier release.

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
4. For a source checkout, build the media companion and choose the camera and
   recording options in the [source station setup](install-from-source.md#start-the-station).
   Start the station with your profile.

   ```sh
   uv run cargo run -p hebbian-rua-server -- serve --profile "$HOME/.config/rua/so101.toml"
   ```

   With an installed wheel, follow the
   [published station setup](getting-started.md#2-verify-manual-control) and use
   `uv run rua serve` with the same `--profile` argument.
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

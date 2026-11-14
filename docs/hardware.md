# Set up each SO-101

Every physical arm needs its own serial port, LeRobot calibration and verified
model mapping. The bundled profile provides starting values, not a calibration
or universal servo tune. Keep the arm supported with torque off during setup,
and keep a physical power cutoff accessible.

1. Follow [LeRobot's SO-101 setup](https://huggingface.co/docs/lerobot/en/so101)
   to identify the port and calibrate with a unique robot ID. The station reads
   `~/.cache/huggingface/lerobot/calibration/robots/so_follower/<id>.json`.
   Prefer a stable `/dev/serial/by-id/` path on Linux.
2. Create a private profile such as `$HOME/.config/rua/so101.toml`:

   ```toml
   extends = "so101"
   name = "my-so101"

   [feetech]
   ports = { follower = "/dev/serial/by-id/your-arm" }
   calibration_ids = { follower = "your-calibration-id" }
   joint_signs = [1, 1, 1, 1, 1]
   joint_offsets_radians = [0.0, 0.0, 0.0, 0.0, 0.0]
   ```

3. With the station stopped and torque off, apply and verify the profile's expected
   servo gains using your hardware setup tools. Station startup verifies the gains
   but does not write them. Recheck them after calibration.
4. Start the station with `uv run rua serve --profile
   "$HOME/.config/rua/so101.toml"`. Before enabling, move each joint gently with
   torque off and compare SDK measured joints and tool poses to the physical
   arm and bundled model. Verify signs, offsets, calibrated endpoints and gripper
   travel on this assembly. RUA does not include a 3D robot viewer; use your
   own inspection tools where needed before motion.

Profile and calibration changes take effect after a station restart. Support
and disable the arm before restarting. Calibrated endpoints define joint limits;
the URDF supplies geometry. Neither establishes physical clearance or support.
Back up profiles and calibration privately; never include them in the public repo.

Before increasing speed, assess target versus measured joint angles, command
outcomes, effort/load, temperature, voltage and clearance. Compare actual PID
registers with the per-arm profile. Do not copy another arm's gains or relax
limits to conceal poor tracking. The controller is soft real time and its
collision checks only cover configured geometry, not unknown workspace objects.

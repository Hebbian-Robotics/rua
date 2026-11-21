# SO-101 dependency

[Hebbian-Robotics/so101](https://github.com/Hebbian-Robotics/so101) owns the hardware
runtime, calibration/setup application, licensed model sources and browser model
helpers. This repository retains the station adapters, motor control and the
larger interactive viewer. Per-arm configs remain in the separate config repo.

## Locate the affected pin

| Component | RUA source of truth |
| --- | --- |
| Rust hardware runtime | Git dependency in `server-core/Cargo.toml`, resolved in `Cargo.lock` |
| Browser model helpers | Git dependency in `package.json`, resolved in `pnpm-lock.yaml` |
| Robot assembly metadata, URDFs, meshes and notices | Git commit and per-file checksums in `robots/so101-assets.toml` |

These pins are independent: a driver-only change does not require a new model
artifact or browser-helper update. Read the manifests for current revisions
rather than maintaining a version list here. Arms are calibrated and their servo
registers written by so101's own setup CLI, which the station does not bundle.

## Make a coordinated update

1. Change and validate the owning SO-101 package using its contributor guide,
   then commit upstream. Do not edit a copied driver, UI or prepared asset here.
2. Make the upstream commit available on SO-101's main branch and update the affected
   Git pins. Asset preparation downloads the source snapshot at that commit.
3. Update affected Cargo, pnpm and uv lockfiles. For model changes, update the
   asset commit and per-file checksums together, then run `uv run rua-dev setup`.
4. Validate the affected RUA boundary: driver/config integration, native setup
   loading, viewer/model loading or geometry checks. Test physical alignment
   separately when the assembly or calibration changes.

Prepared assets live in ignored `robots/so101`; the verified archive cache is
`.cache/so101`. Preparation checks both before reuse. Keep either available for
offline source work. For exact upstream release commands and package validation,
use the [SO-101 contributor guide](https://github.com/Hebbian-Robotics/so101/blob/main/CONTRIBUTING.md).

## Model and distribution boundaries

The SO-101 repository's `robot-assemblies.toml` defines the model files, joint
order, tool frame and jaw-closing axis. RUA profiles select a variant in
`[robot_assembly]` and supply station-specific control settings. A profile can
override individual geometry fields for a local model.

The packaged station includes the manifest, URDFs and meshes. The development
MuJoCo fixture reads its MJCF path and nominal camera board frame from the
prepared manifest in `robots/so101`. It fetches the pinned MJCF when missing
locally and reuses it from a sparse Git checkout in
`.cache/so101-mujoco-model/<commit>/`.

Release preparation includes those exact files in RUA's wheels, with their
license and attribution. Installed RUA never downloads models. Source builds
work offline after setup; keep the prepared directory or cached archive when
working offline. The public source checkout uses the same `rua-dev setup` command and preparation
implementation, without the full-workspace command extensions.

The asset pin includes both the stock model and `robot-soft-gripper-camera.urdf`.
New RUA arm configurations use the TPU gripper and wrist-camera assembly;
`so101` remains available for stock hardware. The configured model supplies
control and collision geometry and is served to the viewer at `/model/<arm>/robot.urdf`.
Source CAD, regeneration and the separate attribution notice live upstream in
[the assembly package](https://github.com/Hebbian-Robotics/so101/blob/main/models/soft-gripper-camera/README.md).
The camera board frame is nominal, not a calibrated optical frame. Verify model
alignment and clearance on the physical assembly before powered operation.

See the [upstream contributor guide](https://github.com/Hebbian-Robotics/so101/blob/main/CONTRIBUTING.md)
for package checks and the [official LeRobot guide](https://huggingface.co/docs/lerobot/so101)
for hardware assembly and setup.

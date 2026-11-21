# Third-party notices

The code in this repository is MIT licensed (see [LICENSE](LICENSE)). The material
below comes from other projects and keeps its own license.

## SO-ARM100 robot model (Apache-2.0)

The stock `robots/so101/robot.urdf` and its original meshes come from the
[TheRobotStudio SO-ARM100](https://github.com/TheRobotStudio/SO-ARM100) repository
(commit `a758567c3978dfeefe282ede0500085a48fe8f78`), licensed under the Apache
License 2.0. The URDF is `Simulation/SO101/so101_new_calib.urdf` with mesh paths
flattened, and the meshes are quadric-decimated copies of its STL files. The
license text is in [SO-101 model license](https://github.com/Hebbian-Robotics/so101/blob/main/setup/src/so101_setup/assets/robot/so101/LICENSE.txt); the
provenance is in [SO-101 model attribution](https://github.com/Hebbian-Robotics/so101/blob/main/setup/src/so101_setup/assets/robot/so101/README.md).

## Optional soft gripper and wrist-camera geometry

`robots/so101/robot-soft-gripper-camera.urdf` adds meshes under
`soft-gripper-camera/`, derived from
[xiehuangbao888/soarm_soft_gripper](https://github.com/xiehuangbao888/soarm_soft_gripper)
at `9ea41e73bf16b6171310913c792bd20ebc6245d3`. That source declares no license;
these derived assets are not covered by the stock model's Apache-2.0 license or
the software MIT license. The bundled `SOFT_GRIPPER_NOTICE.txt` and
[upstream assembly sources](https://github.com/Hebbian-Robotics/so101/tree/7b98bb050b45d5737578340ecaaa3069d5039280/models/soft-gripper-camera)
record their provenance and nominal frame definitions. Redistribution terms
remain unspecified by the source author.

## MuJoCo scene settings (Apache-2.0)

`examples/mujoco_sim/environment.xml` is a floor, light and sky scene for simulated
arms. Its visual, sky texture, floor texture and light settings are taken from
`Simulation/SO101/scene.xml` in the SO-ARM100 repository above; the structure
around them (a scene that attaches one arm model per arm) is authored for this
project.

## Recording schema

`livekit-cameras/schemas/CompressedVideo.bin` contains protobuf descriptors for
`foxglove.CompressedVideo` and `google.protobuf.Timestamp`. The checked-in
descriptor is reproduced byte for byte with `protoc` 29.3 and `--include_imports`
using these definitions from the [Foxglove SDK](https://github.com/foxglove/foxglove-sdk/blob/8dc7525eda8c9b8cbe1b64cc885a14452db832b5/schemas/proto/foxglove/CompressedVideo.proto)
and [Protocol Buffers](https://github.com/protocolbuffers/protobuf/blob/0f8eecfc6b6a5bb16b3761f7f25c26e9c36c5916/src/google/protobuf/timestamp.proto).
Their license texts are included under
[licenses/recording-schema](licenses/recording-schema/).

## Viewer sidebar (MIT)

The viewer sidebar uses components adapted from [shadcn/ui](https://github.com/shadcn-ui/ui).
The [shadcn/ui license](examples/typescript/viewer/public/shadcn-ui-LICENSE.txt)
ships with the viewer assets.

## Dependencies

Source dependencies keep their own licenses. Built distributions can bundle
Rust dependencies and JavaScript from the viewer, and may include or link native
libraries. Review the licenses and required notices for each release artifact:

- Python: `pyproject.toml` and `uv.lock`. Notably, `certifi` and `pathspec` are MPL-2.0.
- Rust (`server-core/`): `Cargo.toml` and `Cargo.lock`. Notably, `serialport` is
  MPL-2.0; `unescaper` and `r-efi` are dual-licensed with a permissive option.
- JavaScript (the viewer): `package.json` and `pnpm-lock.yaml`; all production
  dependencies are permissive (MIT, ISC, BSD, Apache-2.0, 0BSD).

The server-wheel build includes viewer dependency license and notice files under
`share/hebbian-robot-use/licenses/viewer/`, with package names and versions in
`manifest.json`. Copies required because upstream npm packages omit their license
files are documented in [licenses/viewer/README.md](licenses/viewer/README.md).
The wheel build also collects Rust dependency license files and a metadata
inventory under `licenses/rust/`. Entries without license files must be reviewed
before a release; the inventory includes build and other dependency features and
does not establish which native system libraries are present.

The media wheel includes the notices supplied with the linked WebRTC archive
under `share/hebbian-robot-use-media/licenses/webrtc/`. FFmpeg and other native
libraries have their own licenses and must be reviewed for each distribution.

List them with `uv run pip-licenses` (or `uvx pip-licenses --python .venv/bin/python`),
`cargo metadata --format-version 1` and `pnpm licenses list --prod`.

RUA distributions include the model license and attribution beside the bundled
assets in `robots/so101`. Source checkouts prepare them from the checksum-pinned
release in `robots/so101-assets.toml`.

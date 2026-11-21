# Third-party notices

RUA code is MIT licensed. Models and dependencies retain their own licenses.

The SO-101 robot models derive from
[TheRobotStudio/SO-ARM100](https://github.com/TheRobotStudio/SO-ARM100), commit
`a758567c3978dfeefe282ede0500085a48fe8f78`, under Apache-2.0.
See [model attribution](https://github.com/Hebbian-Robotics/so101/blob/main/setup/src/so101_setup/assets/robot/so101/README.md) and its included license files.

The [MuJoCo scene](examples/mujoco_sim/environment.xml) adapts the visual,
floor, sky and light settings from `Simulation/SO101/scene.xml` in the SO-ARM100
repository above. RUA assembles the arm models separately. See the
[Apache-2.0 license](examples/mujoco_sim/LICENSE-APACHE).

Camera components use the separately maintained
[livekit-cameras](https://github.com/Hebbian-Robotics/livekit-cameras) and
[mcap-tools](https://github.com/Hebbian-Robotics/mcap-tools) repositories.
Recording-schema license texts are in [licenses/recording-schema](licenses/recording-schema/).
Additional Rust dependency license sources are in [licenses/rust](licenses/rust/README.md).
The dependency manifests and lockfiles identify the source dependencies and versions.

RUA distributions include the model license and attribution beside the bundled
assets in `robots/so101`. Source checkouts prepare them from the checksum-pinned
release in `robots/so101-assets.toml`.

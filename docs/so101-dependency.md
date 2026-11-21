# SO-101 dependency

RUA uses [Hebbian-Robotics/so101](https://github.com/Hebbian-Robotics/so101)
for the hardware runtime and robot model assets. The upstream setup CLI
handles arm calibration and servo setup.

| What you need to update | Where it is pinned |
| --- | --- |
| Rust hardware runtime | [`server-core/Cargo.toml`](../server-core/Cargo.toml) and `Cargo.lock` |
| Robot assemblies, URDFs and meshes | [`robots/so101-assets.toml`](../robots/so101-assets.toml) |

Update the pin for the change you need. For a runtime update, refresh the
Cargo lockfile and run the affected station tests. For an asset update, update
the revision and file checksums, run `uv run rua-dev setup`, and check the
affected profiles and geometry. The station wheel includes the pinned assets.

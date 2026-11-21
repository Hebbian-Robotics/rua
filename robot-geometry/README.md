# Robot geometry

Internal Rust package for URDF loading, serial-chain forward and inverse
kinematics, collision hull queries and lowest-point calculations. It has no
station, hardware-driver, profile or control-loop dependencies.

`Robot::load` reads geometry and an ordered list of driven revolute joints.
`Robot::serial_chain` builds a tool chain; `IkSolver::new` takes that chain,
joint limits and an explicit seed recovery tolerance. `solve` returns either a
solved pose or a failure reason with the best bounded pose found. Malformed
requests are reported separately as `IkRefusal`.

`SelfCollisionModel` queries one full driven-joint pose using convex hulls. It
excludes shapes on the same or neighboring joints and uses a caller-supplied
penetration allowance. These are discrete geometry queries, not continuous
collision detection or a complete safety system.

RUA keeps configuration adaptation, path sampling, table recovery, command
admission and motion supervision in `controller`. Solver request/outcome types
are owned here and reexported by `contracts`, preserving their serialized form.
The `model` feature is enabled by default; contracts disable it to share only
request/outcome types without pulling geometry dependencies into SDKs.
The controller preserves its existing geometry module paths, adapting its
arm/gripper convention before calling the shared model.

This package stays in the monorepo while its API evolves. Check it independently:

```sh
cargo test -p hebbian-robot-geometry
cargo clippy -p hebbian-robot-geometry --all-targets --all-features
```

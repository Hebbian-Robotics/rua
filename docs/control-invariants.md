# Control invariants and failure responses

The controller checks a plan's structure, admits it against the current arm
state, and checks live feedback during execution. See the
[control model](control-model.md) for commands and lease behavior.

## Plan guarantees

The [path validator](../server-core/src/corner_path.rs) checks that a joint
path has finite values, continuous sections and valid timing and speed.
[Motion admission](../server-core/src/arm/motion_admission.rs) checks arm
limits after rebasing to the current hold or settling pose. The measured start
can use the profile's recovery tolerance. Commanded destinations and subsequent
corners must stay within the joint bounds.

A coordinated step is admitted as a group. If any resource is refused, none of
the step's commands are applied. The
[control loop](../server-core/src/control_loop/mod.rs) checks live tracking and
setpoints throughout execution.

## Geometry guards

The [planner](../server-core/src/planning/mod.rs) checks paths against configured
geometry. The motor loop applies [setpoint guards](../server-core/src/control_loop/guards.rs)
before commanding the arm. Check physical clearance from objects and arms
outside a coordinated step. See [coordinated arm clearance](control-model.md#coordinated-arm-clearance).

### Self-contact

The [self-collision guard](../server-core/src/self_collision.rs) checks modeled
links. A preview can stop at the last clear sample or refuse a path from a
colliding start. Check the model and calibration before changing
[profile settings](../server-core/src/profile/mod.rs) for overlap and excluded
link pairs.

### Table surface guard

Set the table height relative to the arm base. Verify that plane against the
physical setup, especially on a raised mount. The
[surface guard](../server-core/src/surface_guard.rs) checks the lowest modeled
geometry, including the gripper and camera assembly. It blocks further
descent below the configured boundary and allows upward recovery. See the
[SO-101 profile](../profiles/so101-common.toml) for an example configuration.

## Failure responses

| Condition | Response |
| --- | --- |
| Invalid or inadmissible command | Refuse the submission and preserve existing targets. |
| Protective trip with trusted feedback | Stop the affected motion and hold. |
| Missing or malformed feedback | Preserve the last trusted sample. Reconnect and verify a hold or require operator recovery. |
| Hard motor fault | Release torque where appropriate. An unsupported arm can fall. |
| Controller loss | Keep the station available for inspection. The watchdog can supervise a hold and release if required. |

See the [runner](../server-core/src/control_loop/runner.rs),
[recovery checks](../server-core/src/control_loop/recovery.rs), and
[independent guard](../server-core/src/guard.rs) for the exact conditions and
actions. Tests in the [control loop module](../server-core/src/control_loop/)
cover failure handling at the plant boundary.

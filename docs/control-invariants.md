# Control invariants and failure responses

The controller keeps structural validity, arm admission and physical observations
as separate guarantees. See [control model](control-model.md) for command and
lease behavior.

## Plan guarantees

[`ValidatedPlan`](../server-core/src/corner_path.rs) establishes nonempty continuous segments, finite corners of the
expected joint count, a positive finite speed and valid minimum durations. Its
private representation permits checked rebasing without losing those facts.

[`AdmittedPlan`](../server-core/src/arm/motion_admission.rs) additionally checks the arm's speed cap and joint bounds after
rebasing to the current hold or measured settling pose. Only the first corner
may use the configured joint-limit recovery tolerance; subsequent corners must
lie within nominal limits. Group members must contain one straight section.
Arbitrary supplied paths are refused rather than clamped into different paths.
Move and stream reject joint targets outside the configured limits.

Admission is not a permanent collision or tracking certificate. Geometry checks
belong to the planner; current feedback, tracking, collision and surface checks
remain in the control loop. Internally constructed shutdown returns retain their
existing recovery behavior and pass the same per-cycle setpoint guards.

## Geometry guards

The controller samples configured collision geometry during planning and
checks runtime references before writing setpoints. Jaw commands also sample
the opening path. These checks cover modeled geometry. Verify clearance from
workspace objects and arms outside a coordinated step. See
[coordinated arm clearance](control-model.md#coordinated-arm-clearance).

### Self-contact

The bundled SO-101 hardware and simulator profiles enable self-collision
checks. A profile can set the overlap allowance and exclude named link pairs:

```toml
self_collision_check = true
self_collision_penetration_tolerance_meters = 0.01
allowed_collision_pairs = [["first_link", "second_link"]]
```

The allowance permits shallow modeled contact. Geometry on the same joint and
neighboring joints is generally excluded, with an explicit check for the jaw
pair. Use collision-bearing link names for additional exclusions. Each pair
excludes all geometry on those links through their full range of motion.

A preview can report the last clear sample of a limited path. An already
intersecting starting pose may make a path unavailable. Check the model and
calibration before adjusting the allowance.

### Table surface guard

The surface height is measured in the arm’s base frame. For an SO-101 mounted
at tabletop height, the bundled nominal height is zero. Override it for a raised
mount or measured offset:

```toml
[surface_guard]
enabled = true
surface_height_meters = 0.0
leeway_meters = 0.01
```

The guard checks the lowest modeled geometry, including the gripper and camera
assembly. Its lower limit is the configured height minus leeway. It blocks
further descent outside the limit and allows upward recovery. The configured
plane represents the measured setup and requires verification on the physical arm.

## Failure responses

| Condition | Response |
| --- | --- |
| Malformed or inadmissible command | Refuse before changing any target or publishing a command record. |
| A resource in a multi-arm step is refused | Refuse the entire step; other arms and gripper targets remain unchanged. |
| A group has not received every member | Wait while holding; this is an expected intermediate state. |
| A complete group cannot construct its trajectory | Fail every member command and brake to a hold through the existing reference filter. |
| A read fails or positional feedback is malformed | Send no position or gripper target from that reading. Retain the last trusted sample and servo goals, maintain the guard heartbeat, and use the existing communication grace and recovery policy. |
| Enable or recovery feedback is malformed | Refuse confirmation; do not create a controller or measured hold from that reading. Preflight and recovery approval validate before taking torque up. |
| A path calculation fails with trusted feedback | End the activity through the existing trip path and hold at the valid measured pose. |
| A gripper control calculation or internal controller invariant fails | Fail its command, hold the gripper at the valid measured angle and trip the arm. A failure to write that hold follows the communication policy. |
| A real gripper write fails | Follow the existing communication grace and reconnect policy. |
| The independent guard receives malformed positional feedback | Refuse the supervised hold and run its release-and-confirm cleanup. Report any inability to confirm release. |

Positional feedback requires exactly the arm's joint count, finite joint
positions and a finite gripper angle when present. A configured gripper must
provide its measurement; an absent controller is an internal control failure. This is checked before
observation, drift comparisons or setpoint generation. A failed read buffer is
unspecified and must never replace the last trusted sample. Healthy cycles swap
reusable buffers instead of cloning telemetry or adding a per-cycle allocation.

Release does not silently become a universal error fallback: an unsupported arm
may fall when torque is removed. Communication loss preserves the established
hold-and-recover policy; physical evidence continues to determine hard faults.
No failure response loosens limits, gains or collision margins.

## Domain values and decision boundaries

Admitted gripper targets carry finite `JointAngle` and positive finite
`JointSpeed` values. `GroupId` is distinct from command identifiers; existing
controller instance and request identifiers also retain their distinct types.
Wire contracts and numerical kernels continue to use their established SI
representations, with explicit conversion at those boundaries. Monotonic
intervals use the standard time types where possible.

Motion and action-step preparation only read state and return owned admission
decisions. All resources prepare before the imperative shell applies them.
Applying those decisions changes records, queues and targets without repeating
validation. The shell retains ordered event publication before request replies;
there is no general effect framework, state cloning or hardware I/O in the
preparation logic.

## Property testing

The [arm properties](../server-core/src/arm/tests.rs) and
[lease properties](../server-core/src/lease.rs) cover terminal records, refusal
without target changes and stale-token fencing. The
[lifecycle tests](../server-core/src/control_loop/lifecycle_tests.rs) exercise
malformed feedback at the actuator boundary. Hardware behavior requires
validation on the physical arm.

# SO-101 self-clipping checks

The SO-101 viewer and motion core use the URDF's collision meshes in the Rust
executor: the convex hull of each mesh, checked with parry3d (`server-core/src/self_collision.rs`). They allow contact and a configurable overlap allowance;
the goal is to prevent clear self-interpenetration, not contact-free motion.
Meshes on the same rigid link and directly connected arm links are excluded
because those parts meet by design. The moving/fixed jaw pair is checked explicitly.

```toml
self_collision_check = true
self_collision_penetration_tolerance_meters = 0.01
```

The stock SO-101 and simulation profiles enable this check.
A local profile can override the allowance for that arm/model.
This allowance is a geometric model parameter, separate from calibrated joint
limits and tracking/effort guards.

## Behavior

- The preview checks the joint path from measured joints to the proposed pose,
  including the explicit wrist override. On a blocked path it retains the last
  accepted sample as a constrained ghost, with the clipping reason.
- One-shot motion plans are rejected before queueing if their joint segments
  clip. Runtime references from every controller are checked again before
  writing setpoints, so streamed destinations and policy references share the
  guard. A runtime trip uses the existing measured-pose hold behavior.
- A measured starting pose that already exceeds the model allowance cannot
  certify a path. The viewer reports this explicitly rather than approving an
  intersecting ghost. Check the model/calibration and configured allowance for
  a physically valid resting pose. Physical support does not prove that a modeled
pose meets the configured overlap allowance.
- Collision geometry is prepared before control starts, avoiding mesh loading
  in the first motor-control cycle.

## Allowance selection

The default allowance accommodates modeled contact on the bundled geometry; it
is not a calibration or guarantee for every SO-101 assembly. Check the model
and the actual arm before changing it.

## Scope

[The executor samples joint paths](../server-core/src/self_collision.rs) against
the arm's configured self-geometry. Thin intersections can fall between samples.
Workspace objects and separately modeled arms are outside these checks. The
[surface guard](reference.md#table-surface-guard) checks its configured plane
separately.

## Gripper checks

Runtime arm and jaw commands also check the measured jaw opening, mapped to
the selected model’s closed/open angles. Jaw commands
sample the opening path. The adjacent moving/fixed jaw pair is checked while
other adjacent arm joints retain their exclusions. The same overlap allowance
applies, so contact and small model differences remain permitted. A preview
covers the jaw configuration used for that request. Check a later gripper
command separately. Startup telemetry is not a physical collision diagnosis.

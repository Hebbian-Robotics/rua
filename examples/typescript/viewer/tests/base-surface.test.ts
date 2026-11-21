import * as THREE from "three";
import { test } from "vitest";
import { intersectsBaseSurface } from "../src/base-surface.js";

test("base surface", async () => {
  function assert(condition: unknown, message: string): asserts condition {
    if (!condition) throw new Error(message);
  }
  function link(name: string): THREE.Group {
    const group = new THREE.Group();
    group.name = name;
    Object.assign(group, { isURDFLink: true });
    return group;
  }
  const robot = link("robot");
  const base = link("base_link");
  const shoulder = link("shoulder_link");
  robot.add(base);
  base.add(shoulder);
  const baseMesh = new THREE.Mesh(new THREE.BoxGeometry(0.2, 0.2, 0.2));
  base.add(baseMesh);
  const movingMesh = new THREE.Mesh(new THREE.BoxGeometry(0.1, 0.1, 0.1));
  movingMesh.position.z = 0.1;
  shoulder.add(movingMesh);
  assert(
    !intersectsBaseSurface(robot, 0),
    "Fixed base below its mounting plane must be allowed",
  );
  const jaw = link("moving_jaw_so101_v1_link");
  shoulder.add(jaw);
  const jawMesh = new THREE.Mesh(new THREE.BoxGeometry(0.02, 0.02, 0.02));
  jawMesh.position.z = -0.02;
  jaw.add(jawMesh);
  assert(
    intersectsBaseSurface(robot, 0),
    "The moving jaw must be checked against the surface",
  );
  jawMesh.position.z = 0.1;
  movingMesh.position.z = 0.04;
  assert(
    intersectsBaseSurface(robot, 0),
    "A moving link crossing the base plane must be blocked",
  );
  assert(
    !intersectsBaseSurface(robot, -0.1),
    "A lower surface allows the same pose",
  );
  movingMesh.position.z = 0.2;
  shoulder.rotation.y = Math.PI / 2;
  assert(
    intersectsBaseSurface(robot, 0),
    "Transformed mesh vertices must be checked in world coordinates",
  );
  assert(
    intersectsBaseSurface(robot, Number.NaN),
    "An invalid surface height must fail closed",
  );
  assert(
    intersectsBaseSurface(new THREE.Group(), 0),
    "An unavailable mesh must fail closed",
  );
});

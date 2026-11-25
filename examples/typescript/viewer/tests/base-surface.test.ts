import * as THREE from "three";
import { expect, test } from "vitest";
import { intersectsBaseSurface } from "../src/base-surface.js";

function urdfLink(name: string): THREE.Group {
  const group = new THREE.Group();
  group.name = name;
  return Object.assign(group, { isURDFLink: true });
}

function robotWithMovingLink() {
  const robot = urdfLink("robot");
  const base = urdfLink("base_link");
  const shoulder = urdfLink("shoulder_link");
  robot.add(base);
  base.add(shoulder);
  base.add(new THREE.Mesh(new THREE.BoxGeometry(0.2, 0.2, 0.2)));
  const shoulderMesh = new THREE.Mesh(new THREE.BoxGeometry(0.1, 0.1, 0.1));
  shoulderMesh.position.z = 0.1;
  shoulder.add(shoulderMesh);
  return { robot, shoulder, shoulderMesh };
}

test("the fixed base is allowed below the surface while hand and arm geometry are checked", () => {
  const { robot, shoulder, shoulderMesh } = robotWithMovingLink();
  expect(intersectsBaseSurface(robot, 0)).toBe(false);

  const movingJaw = urdfLink("moving_jaw_so101_v1_link");
  const jawMesh = new THREE.Mesh(new THREE.BoxGeometry(0.02, 0.02, 0.02));
  jawMesh.position.z = -0.02;
  movingJaw.add(jawMesh);
  shoulder.add(movingJaw);
  expect(intersectsBaseSurface(robot, 0)).toBe(true);

  jawMesh.position.z = 0.1;
  shoulderMesh.position.z = 0.04;
  expect(intersectsBaseSurface(robot, 0)).toBe(true);
  expect(intersectsBaseSurface(robot, -0.1)).toBe(false);
});

test("a link intersects according to its world coordinates after rotation", () => {
  const { robot, shoulder, shoulderMesh } = robotWithMovingLink();
  shoulderMesh.position.z = 0.2;
  expect(intersectsBaseSurface(robot, 0)).toBe(false);
  shoulder.rotation.y = Math.PI / 2;
  expect(intersectsBaseSurface(robot, 0)).toBe(true);
});

test("missing geometry or an invalid surface blocks motion", () => {
  const { robot } = robotWithMovingLink();
  expect(intersectsBaseSurface(robot, Number.NaN)).toBe(true);
  expect(intersectsBaseSurface(new THREE.Group(), 0)).toBe(true);
});

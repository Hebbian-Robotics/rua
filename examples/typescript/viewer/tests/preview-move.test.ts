import { expect, test } from "vitest";
import {
  describeApproach,
  moveCommandForPreview,
  movePreviewedGripper,
  syncCurrentPosePreview,
  unreachableYawLines,
} from "../src/preview-move.js";

test("the previewed gripper opening waits for admission of the arm move", async () => {
  const openings: number[] = [];
  let finishMove: (accepted: boolean) => void = () => {};
  const move = new Promise<boolean>((resolve) => {
    finishMove = resolve;
  });
  const pending = movePreviewedGripper(
    () => move,
    () => true,
    true,
    0.7,
    (opening) => openings.push(opening),
  );
  expect(openings).toEqual([]);
  finishMove(true);
  await pending;
  expect(openings).toEqual([0.7]);
});

test.each([
  { admitted: false, stillMoving: true, drivesGripper: true },
  { admitted: true, stillMoving: false, drivesGripper: true },
  { admitted: true, stillMoving: true, drivesGripper: false },
])(
  "no gripper move after admission=$admitted, active=$stillMoving, gripper=$drivesGripper",
  async ({ admitted, stillMoving, drivesGripper }) => {
    const openings: number[] = [];
    await movePreviewedGripper(
      async () => admitted,
      () => stillMoving,
      drivesGripper,
      0.2,
      (opening) => openings.push(opening),
    );
    expect(openings).toEqual([]);
  },
);

test("the target copies the measured position and gripper opening only when available", () => {
  const openings: number[] = [];
  const positions: number[][] = [];
  const setOpening = (opening: number) => openings.push(opening);
  const setPosition = (position: number[]) => positions.push(position);
  expect(
    syncCurrentPosePreview(
      {
        tool: {
          position: [0.1, 0.2, 0.3],
          quaternion: [0, 0, 0, 1],
          orientation: {
            approach: [0, 0, -1],
            radians_from_down: 0,
            jaw_yaw_radians: 0,
            jaw_yaw_reference: "world_x",
            goal: null,
          },
          model_points: [],
        },
        gripper_opening: 0.42,
      },
      setOpening,
      setPosition,
    ),
  ).toBe(true);
  expect(
    syncCurrentPosePreview(
      { tool: null, gripper_opening: null },
      setOpening,
      setPosition,
    ),
  ).toBe(false);
  expect(openings).toEqual([0.42]);
  expect(positions).toEqual([[0.1, 0.2, 0.3]]);
});

test("orientation labels describe the custom approach and preserve unreachable yaw reasons", () => {
  const ranges = [
    {
      from_radians: 0,
      to_radians: (20 * Math.PI) / 180,
      reachable: false,
      reason: "Wrist limit",
    },
    {
      from_radians: (20 * Math.PI) / 180,
      to_radians: (340 * Math.PI) / 180,
      reachable: true,
      reason: null,
    },
    {
      from_radians: (340 * Math.PI) / 180,
      to_radians: 2 * Math.PI,
      reachable: false,
      reason: "Wrist limit",
    },
  ];
  expect(unreachableYawLines(ranges)).toEqual([
    "0°–20°: Wrist limit",
    "340°–360°: Wrist limit",
  ]);
  expect(describeApproach([0.6, 0, -0.8])).toBe("custom (0.60, 0.00, -0.80)");
});

test("IK moves to the preview's solved joints while policies move to the requested pose", () => {
  const preview = {
    joints: [0.1, 0.2, 0.3, 0.4, 0.5],
    position: [0.25, 0, 0.1],
    quaternion: [0, 0, 0, 1],
    requestedPosition: [0.3, 0, 0.1],
    requestedQuaternion: [0, 1, 0, 0],
  };
  expect(moveCommandForPreview("ik", preview, 0.5)).toMatchObject({
    kind: "move_to_joints",
    joints: [0.1, 0.2, 0.3, 0.4, 0.5],
  });
  expect(moveCommandForPreview("policy", preview, 0.5)).toMatchObject({
    kind: "move_to_pose",
    position: [0.3, 0, 0.1],
    orientation: { kind: "quaternion", quaternion: [0, 1, 0, 0] },
  });
  expect(
    moveCommandForPreview(
      "policy",
      { ...preview, requestedQuaternion: null },
      0.5,
    ),
  ).toBeNull();
});

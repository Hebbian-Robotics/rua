import { expect, test } from "vitest";

import { armMotionCommands } from "../src/index.js";

test("a command method sends its command, with absent options as null", () => {
  const sent: unknown[] = [];
  const arm = armMotionCommands((command) => sent.push(command));

  arm.moveToJoints([0.1, 0.2]);
  arm.moveToJoints([0.1, 0.2], 0.5);
  arm.setGripper(1);

  expect(sent).toEqual([
    { kind: "move_to_joints", joints: [0.1, 0.2], max_joint_speed: null },
    { kind: "move_to_joints", joints: [0.1, 0.2], max_joint_speed: 0.5 },
    { kind: "set_gripper", opening: 1, max_joint_speed: null },
  ]);
});

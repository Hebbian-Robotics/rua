import type { CommandRecord } from "@hebbian/robot-use-client";
import { describe, expect, it } from "vitest";
import { describeCommandOutcome } from "../src/command-outcome.js";

function stoppedMove(jointErrors: number[] | null): CommandRecord {
  return {
    label: "move_to_joints",
    completion: {
      finished_at: 1,
      outcome: "stopped_short",
      detail: "Stopped short",
    },
    achieved: { joint_errors_radians: jointErrors },
  } as CommandRecord;
}

describe("manual command outcomes", () => {
  it("shows a small measured miss as approximate, with its maximum error", () => {
    expect(
      describeCommandOutcome(stoppedMove([0.007, 0.008, 0.043, 0.006, 0.009])),
    ).toEqual({
      text: "Last: move_to_joints reached approximately — maximum joint error 2.5°",
      isError: false,
    });
  });

  it("shows a small pose miss as approximate but preserves a trip", () => {
    const command = { ...stoppedMove([0.043]), label: "move_to_pose" };
    expect(describeCommandOutcome(command)).toEqual({
      text: "Last: move_to_pose reached approximately — maximum joint error 2.5°",
      isError: false,
    });
    expect(
      describeCommandOutcome({
        ...command,
        completion: {
          finished_at: 1,
          outcome: "tripped",
          detail: "Loop deadline missed",
          gripper: null,
        },
      }),
    ).toEqual({
      text: "Last: move_to_pose tripped — Loop deadline missed",
      isError: true,
    });
  });

  it("keeps a larger or unknown miss as stopped short", () => {
    for (const errors of [[0.061], null, []]) {
      expect(describeCommandOutcome(stoppedMove(errors))).toEqual({
        text: "Last: move_to_joints stopped short — Stopped short",
        isError: true,
      });
    }
  });
});

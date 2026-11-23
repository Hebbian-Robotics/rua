import { apiSchemas, type CommandRecord } from "hebbian-rua";
import { describe, expect, it } from "vitest";
import { describeCommandOutcome } from "../src/command-outcome.js";

function stoppedMove(jointErrors: number[] | null): CommandRecord {
  return apiSchemas.shape.CommandRecord.parse({
    command_id: "move",
    label: "move_to_joints",
    accepted_unix_nano: "0",
    checkpoints: [],
    trace_context: null,
    command: {
      kind: "move",
      destination_joints: [0.1],
      joint_speed: 1,
      group_id: null,
      destination_position: null,
      destination_quaternion: null,
      path_limited: null,
      pose_limited: null,
    },
    phase: {
      kind: "finished",
      completion: {
        finished_unix_nano: "1000000000",
        outcome: "stopped_short",
        detail: "Stopped short",
        gripper: null,
      },
      achieved: {
        joints: [0.07],
        sampled_at: 1,
        joint_errors_radians: jointErrors,
        joint_error_radians: null,
        orientation: null,
        orientation_error_radians: null,
        position: null,
        position_error_meters: null,
        quaternion: null,
      },
    },
  });
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
        phase: {
          kind: "finished",
          achieved: null,
          completion: {
            finished_unix_nano: "1000000000",
            outcome: "tripped",
            detail: "Loop deadline missed",
            gripper: null,
          },
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

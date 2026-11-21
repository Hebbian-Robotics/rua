import { expect, test, vi } from "vitest";
import {
  apiSchemas,
  createRobotTools,
  createStationConnection,
  defaultFormatter,
  fullFormatter,
  type OutputFormatter,
  type RobotOutput,
  type RobotToolCall,
  renderOutput,
  selectOutputCameras,
  summarizeOutput,
} from "../src/index.js";
import { commandRecord, fakeLease, stateMessage } from "./station-fakes.js";

test("formatting views retain full motion history, units and interruption flags", async () => {
  const record = apiSchemas.shape.CommandRecord.parse({
    ...commandRecord("move", 2, "interrupted"),
    achieved: {
      joints: [0.123456789],
      sampled_at: 2.123456789,
      joint_error_radians: null,
      joint_errors_radians: null,
      orientation: null,
      orientation_error_radians: null,
      position: null,
      position_error_meters: null,
      quaternion: null,
    },
    checkpoints: [
      {
        label: "start",
        measured_joints: [0.123456789],
        target_position: null,
        target_quaternion: null,
      },
    ],
  });
  const connection = createStationConnection("http://station", "agent");
  const robot = createRobotTools({
    connection,
    lease: fakeLease({
      runCommand: async () => ({
        status: "accepted",
        records: [{ arm: "follower", record }],
      }),
    }),
    arm: "follower",
  });
  const calls: RobotToolCall[] = [
    {
      id: "close",
      name: "stop",
      arguments: {},
    },
  ];
  try {
    const [summary] = await robot.execute(calls);
    expect(summary?.data).toEqual(record);
    expect(summary?.modelData).toMatchObject({
      achieved: { joints_degrees: [7.1] },
    });
    expect(summary?.modelData).not.toHaveProperty("checkpoints");
    const [full] = await robot
      .withOutputFormatter(fullFormatter)
      .execute(calls);
    expect(full?.modelData).toEqual(record);
    expect(full?.data).toEqual(summary?.data);
    expect(full).toMatchObject({
      isError: true,
      interrupted: true,
      halt: false,
    });

    // Each view receives the full payload, even after another formatter was selected.
    const custom: OutputFormatter = (output) => {
      const content = defaultFormatter(output);
      if (typeof output.data === "object" && output.data !== null)
        Object.assign(output.data, { label: "Changed by formatter" });
      return { ...content, text: `Task view: ${content.text}` };
    };
    const [mapped] = await robot
      .withOutputFormatter(fullFormatter)
      .withOutputFormatter(custom)
      .mapTools((capabilities) => capabilities.toReversed())
      .execute(calls);
    expect(mapped?.text).toContain('"joints_degrees":[7.1]');
    expect(mapped?.modelData).toEqual(mapped?.text);
    expect(mapped?.data).toEqual(record);
    const [original] = await robot.execute(calls);
    expect(original?.modelData).toEqual(summary?.modelData);

    let completedCommands = 0;
    const broken = robot.withOutputFormatter(() => {
      throw new Error("Formatter failed");
    });
    await expect(
      broken.execute(calls, undefined, async (_call, run) => {
        const result = await run();
        completedCommands++;
        return result;
      }),
    ).rejects.toThrow("Formatter failed");
    expect(completedCommands).toBe(1);
  } finally {
    robot.close();
  }
});

test("output transforms compose before rendering without changing the source data or cameras", () => {
  const output: RobotOutput = {
    kind: "observation",
    data: {
      checkpoints: [{ label: "start", measured_joints: [0.123456789] }],
      achieved: { joints: [0.123456789] },
      completion: { outcome: "interrupted", detail: "Controller disconnected" },
    },
    selectedArm: "follower",
    notes: ["Recovered connection"],
    cameras: {
      table: new Uint8Array([1]),
      side: new Uint8Array([2]),
    },
    missingOptionalCameras: ["wrist"],
  };
  const transformed = selectOutputCameras(summarizeOutput(output), ["table"]);
  expect(transformed.data).toEqual({
    achieved: { joints: [0.123456789] },
    completion: { outcome: "interrupted", detail: "Controller disconnected" },
  });
  const content = renderOutput(transformed);
  expect(content.text).toContain('"joints_degrees":[7.1]');
  expect(content.text).toContain("Controller disconnected");
  expect(content.text).toContain("Recovered connection");
  expect(content.text).toContain("Optional cameras unavailable: wrist");
  expect(content.cameras).toEqual({ table: new Uint8Array([1]) });
  expect(fullFormatter(output).text).toContain('"joints":[0.123456789]');
  expect(fullFormatter(output).text).toContain('"checkpoints":[{');
  expect(output.cameras).toEqual({
    table: new Uint8Array([1]),
    side: new Uint8Array([2]),
  });
});

test("observation views select presentation while retaining complete station data", async () => {
  const connection = createStationConnection("http://station", "agent");
  const state = apiSchemas.shape.StationState.parse(stateMessage([{}]).state);
  const arm = state.arms[0];
  if (!arm) throw new Error("Missing fixture arm");
  arm.measured_joints = [0.123456789];
  arm.measured_joint_velocities = [0.5];
  const stateSpy = vi
    .spyOn(connection, "waitForState")
    .mockResolvedValue(state);
  const robot = createRobotTools({
    connection,
    lease: fakeLease(),
    arm: "follower",
  });
  try {
    const summary = await robot.observe();
    expect(summary.data).toEqual(state);
    expect(summary.text).toContain('"measured_joints_degrees":[7.1]');
    expect(summary.text).not.toContain("measured_joint_velocities");
    const full = await robot.withOutputFormatter(fullFormatter).observe();
    expect(full.data).toEqual(state);
    expect(full.text).toContain('"measured_joints":[0.123456789]');
    expect(full.text).toContain('"measured_joint_velocities":[0.5]');
    expect(full.revisions).toEqual(summary.revisions);
  } finally {
    robot.close();
    stateSpy.mockRestore();
  }
});

test("camera selection filters historical images as well as current images", () => {
  const output: RobotOutput = {
    kind: "observation",
    data: {},
    selectedArm: "follower",
    notes: [],
    cameras: { front: new Uint8Array([9]) },
    missingOptionalCameras: [],
    sequenceImages: {
      callId: "move",
      missedSamples: 0,
      truncated: false,
      images: [
        {
          camera: "front",
          jpeg: new Uint8Array([1, 2, 3]),
          elapsedSeconds: 0,
          retrievedBetweenUnixSeconds: [100, 101],
          boundary: "start",
        },
      ],
    },
  };
  const rendered = renderOutput(selectOutputCameras(output, ["front"]));
  expect(rendered.cameras).toEqual({
    front: new Uint8Array([9]),
    "Sequence move image 1: front, +0.000s, start; retrieved 100–101 Unix seconds":
      new Uint8Array([1, 2, 3]),
  });
  expect(rendered.text).toContain("historical");
  expect(renderOutput(selectOutputCameras(output, [])).cameras).toEqual({});
});

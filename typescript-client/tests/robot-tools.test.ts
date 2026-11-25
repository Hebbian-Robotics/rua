import { readFileSync } from "node:fs";
import { expect, test, vi } from "vitest";
import { apiSchemas } from "../src/generated/api-schemas.js";
import {
  angularDegrees,
  angularRadians,
  createRobotTools,
  createStationConnection,
  filterSequenceActions,
  jointRadians,
  positionMetres,
} from "../src/index.js";
import { parseRobotToolCall } from "../src/robot-tool-input.js";
import { fakeLease, stateMessage } from "./station-fakes.js";

const connection = createStationConnection("http://localhost", "agent");
const robot = createRobotTools({
  tools: [
    "action_sequence",
    "preview_pose",
    "move_arms",
    "set_arm_gripper",
    "rehome",
    "stop",
    "observe",
  ],
  connection,
  lease: fakeLease(),
  arm: "follower",
});

test("public SDK units produce the same station inputs", () => {
  expect(positionMetres([200, -100, 50], "mm")).toEqual([0.2, -0.1, 0.05]);
  expect(positionMetres([0.2, -0.1, 0.05], "m")).toEqual([0.2, -0.1, 0.05]);
  expect(angularRadians(90, "deg")).toBeCloseTo(Math.PI / 2);
  expect(angularDegrees(Math.PI / 2, "rad")).toBeCloseTo(90);
  expect(jointRadians([90, -180], "deg")).toEqual([Math.PI / 2, -Math.PI]);
});

test("tool input units match the station contract", () => {
  const fixture = JSON.parse(
    readFileSync(
      new URL("../../tests/fixtures/model-input-units.json", import.meta.url),
      "utf8",
    ),
  );
  const parsed = parseRobotToolCall(
    { id: "units", name: "move_arms", arguments: fixture.arguments },
    {
      arm: "position",
      sequenceScope: { kind: "single" },
      sequenceActions: new Set(),
      availableToolNames: new Map([["move_arms", "move_arms"]]),
    },
  );
  expect(parsed).toMatchObject({
    kind: "message",
    message: { kind: "move_arms", command: fixture.station_command },
  });
});

test("timed sequences convert angles and retain waypoint and goal seconds", () => {
  const parsed = parseRobotToolCall(
    {
      id: "timed-move",
      name: "action_sequence",
      arguments: {
        steps: [
          {
            waypoints: [
              {
                at: { joints_degrees: [45, 0, 0] },
                at_seconds: 1.25,
              },
            ],
            goal: { joints_degrees: [90, 0, 0] },
            at_seconds: 2.5,
          },
        ],
      },
    },
    {
      arm: "follower",
      sequenceScope: { kind: "single" },
      sequenceActions: new Set(["joints"]),
      availableToolNames: new Map([["action_sequence", "action_sequence"]]),
    },
  );
  expect(parsed).toMatchObject({
    kind: "message",
    message: {
      kind: "start_action_sequence",
      action_sequence: {
        steps: [
          {
            arms: {
              follower: {
                waypoints: [
                  {
                    at: { kind: "joints", joints: [Math.PI / 4, 0, 0] },
                    at_seconds: 1.25,
                  },
                ],
                goal: { kind: "joints", joints: [Math.PI / 2, 0, 0] },
                at_seconds: 2.5,
              },
            },
          },
        ],
      },
    },
  });
});

test("pose previews convert degree-based joint caps to station units", () => {
  const parsed = parseRobotToolCall(
    {
      id: "preview-caps",
      name: "preview_pose",
      arguments: {
        poses: [
          {
            pose: {
              position_mm: [200, 0, 100],
              max_joint_acceleration_degrees_per_second_squared: 90,
              max_joint_jerk_degrees_per_second_cubed: 180,
            },
          },
        ],
      },
    },
    {
      arm: "follower",
      sequenceScope: { kind: "single" },
      sequenceActions: new Set(),
      availableToolNames: new Map([["preview_pose", "preview_pose"]]),
    },
  );
  expect(parsed).toMatchObject({
    kind: "preview",
    poses: [
      {
        pose: {
          position: [0.2, 0, 0.1],
          max_joint_acceleration: Math.PI / 2,
          max_joint_jerk: Math.PI,
        },
      },
    ],
  });
});

test("model tools return invalid inputs as errors to the model", async () => {
  const results = await robot.execute([
    { id: "1", name: "action_sequence", arguments: { steps: [] } },
    {
      id: "2",
      name: "set_arm_gripper",
      arguments: { arm: "follower", opening: "half" },
    },
  ]);
  expect(results).toHaveLength(2);
  for (const result of results) {
    expect(result.isError).toBe(true);
    expect(result.text).toContain("Invalid input");
  }
});

test.each([false, true])(
  "required camera recovery returns a fresh observation (already interlocked: %s)",
  async (alreadyInterlocked) => {
    const { vi } = await import("vitest");
    const { createRobotTools, createStationConnection } = await import(
      "../src/index.js"
    );
    const connection = createStationConnection("http://station", "agent");
    let revision = 0;
    let interlocks: { kind: string; camera: string }[] = alreadyInterlocked
      ? [{ kind: "required_camera_unavailable", camera: "front" }]
      : [];
    let cameraAvailable = alreadyInterlocked;
    const state = () =>
      apiSchemas.shape.StationState.parse({
        ...stateMessage([{ revision }]).state,
        robot: "sim",
        arms: stateMessage([{ revision }]).state.arms.map((arm) => ({
          ...arm,
          interlocks: interlocks.map((interlock) => ({
            ...interlock,
            detail: "missing",
          })),
        })),
      });
    vi.spyOn(connection, "getState").mockImplementation(state);
    vi.spyOn(connection, "waitForState").mockImplementation(async () => {
      const current = state();
      if (current === null) throw new Error("Expected station state");
      return current;
    });
    const reports: unknown[] = [];
    const lease = fakeLease({
      reportCamera: async (_camera, available) => {
        reports.push({ available });
        if (available) interlocks = [];
        else {
          interlocks = [
            { kind: "required_camera_unavailable", camera: "front" },
          ];
          revision++;
          cameraAvailable = true;
        }
      },
    });
    vi.stubGlobal("fetch", async (input: URL | string) => {
      const url = new URL(String(input));
      if (url.pathname === "/observations")
        return Response.json(
          cameraAvailable
            ? {
                status: "observation_captured",
                state: state(),
                cameras: {
                  front: {
                    jpeg_base64: "AQID",
                    captured_at_unix_ns: null,
                    retrieved_between_unix_seconds: [0, 0],
                  },
                },
                missing_optional_cameras: [],
              }
            : {
                status: "observation_required_camera_unavailable",
                camera: "front",
              },
        );
      throw new Error(`Unexpected URL ${url}`);
    });
    const robot = createRobotTools({
      tools: [
        "action_sequence",
        "preview_pose",
        "move_arms",
        "set_arm_gripper",
        "rehome",
        "stop",
        "observe",
      ],
      connection,
      lease,
      arm: "follower",
      cameras: ["front"],
    });
    try {
      const observation = await robot.observeWhenReady();
      expect(observation.revisions).toEqual({
        follower: alreadyInterlocked ? 0 : 1,
      });
      expect(observation.cameras["front"]).toEqual(new Uint8Array([1, 2, 3]));
      expect(reports).toEqual(
        alreadyInterlocked
          ? [{ available: true }]
          : [{ available: false }, { available: true }],
      );
      expect(interlocks).toEqual([]);
    } finally {
      robot.close();
      vi.unstubAllGlobals();
      vi.restoreAllMocks();
    }
  },
);

test.each([
  { reason: "stale_revision", halt: false, interrupted: true },
  { reason: "interlocked", halt: false, interrupted: true },
  { reason: "lease_held_elsewhere", halt: false, interrupted: true },
  { reason: "faulted", halt: true, interrupted: false },
  { reason: "malformed", halt: false, interrupted: false },
] as const)(
  "classifies a $reason refusal for the model",
  async ({ reason, halt, interrupted }) => {
    const connection = createStationConnection("http://station", "agent");
    const lease = fakeLease({
      runCommand: async () => ({
        status: "refused",
        reason,
        detail: "The station refused the command",
      }),
    });
    const robot = createRobotTools({
      tools: [
        "action_sequence",
        "preview_pose",
        "move_arms",
        "set_arm_gripper",
        "rehome",
        "stop",
        "observe",
      ],
      connection,
      lease,
      arm: "follower",
    });
    try {
      const [stopped] = await robot.execute(
        [
          {
            id: "1",
            name: "set_arm_gripper",
            arguments: { arm: "follower", opening: 1 },
          },
        ],
        { follower: 3 },
      );
      expect(stopped).toMatchObject({
        isError: true,
        halt,
        interrupted,
        data: { reason },
      });
    } finally {
      robot.close();
      vi.restoreAllMocks();
    }
  },
);

test("model responses show readable units and preserve copyable orientations", async () => {
  const { modelResponse } = await import("../src/index.js");
  const fixtures = JSON.parse(
    readFileSync(
      new URL("../../tests/fixtures/model-response.json", import.meta.url),
      "utf8",
    ),
  ) as { input: unknown; expected: unknown }[];
  for (const fixture of fixtures)
    expect(modelResponse(fixture.input)).toEqual(fixture.expected);
});

test("concise views match the shared examples", async () => {
  const { conciseOutput, conciseResponse } = await import("../src/index.js");
  const fixtures = JSON.parse(
    readFileSync(
      new URL("../../tests/fixtures/model-concise.json", import.meta.url),
      "utf8",
    ),
  ) as {
    name: string;
    keep_station_context: boolean;
    input: unknown;
    concise: unknown;
    expected: unknown;
  }[];
  for (const fixture of fixtures) {
    const options = { keepStationContext: fixture.keep_station_context };
    expect(conciseOutput(fixture.input, options), fixture.name).toEqual(
      fixture.concise,
    );
    expect(conciseResponse(fixture.input, options), fixture.name).toEqual(
      fixture.expected,
    );
  }
});

test("group outcomes match the shared examples", async () => {
  const examples = JSON.parse(
    readFileSync(
      new URL(
        "../../tests/fixtures/model-command-outcomes.json",
        import.meta.url,
      ),
      "utf8",
    ),
  ) as {
    name: string;
    record: unknown;
    expected_error: boolean;
    expected_status: string;
  }[];
  for (const example of examples) {
    const connection = createStationConnection("http://station", "agent");
    const record = apiSchemas.shape.CommandRecord.parse(example.record);
    const robot = createRobotTools({
      tools: [
        "action_sequence",
        "preview_pose",
        "move_arms",
        "set_arm_gripper",
        "rehome",
        "stop",
        "observe",
      ],
      connection,
      lease: fakeLease({
        runGroup: async () => ({
          status: "accepted",
          records: [{ arm: "follower", record }],
        }),
        runCommand: async () => ({
          status: "accepted",
          records: [{ arm: "follower", record }],
        }),
      }),
      arm: "follower",
    });
    try {
      const [result] = await robot.execute([
        {
          id: example.name,
          name: "move_arms",
          arguments: {
            goals: { follower: { joints_degrees: [0, 0, 0, 0, 0] } },
          },
        },
      ]);
      expect(result?.isError).toBe(example.expected_error);
      expect(result?.data).toMatchObject({ status: example.expected_status });
      const [single] = await robot.execute([
        {
          id: example.name,
          name: "set_arm_gripper",
          arguments: { arm: "follower", opening: 0.5 },
        },
      ]);
      expect(single?.isError).toBe(example.expected_error);
      expect(single?.data).toMatchObject({ phase: record.phase });
    } finally {
      robot.close();
    }
  }
});

test("tool calls run concurrently and results retain input order", async () => {
  const connection = createStationConnection("http://station", "agent");
  const started: number[] = [];
  let releaseFirst: (() => void) | undefined;
  const firstFinished = new Promise<void>((resolve) => {
    releaseFirst = resolve;
  });
  const lease = fakeLease({
    runCommand: async (_arm, command) => {
      if (command.kind !== "set_gripper") throw new Error("unexpected command");
      started.push(command.opening);
      if (command.opening === 0.1) await firstFinished;
      return {
        status: "refused",
        reason: command.opening === 0.2 ? "stale_revision" : "malformed",
        detail: "refused",
      };
    },
  });
  const robot = createRobotTools({
    tools: [
      "action_sequence",
      "preview_pose",
      "move_arms",
      "set_arm_gripper",
      "rehome",
      "stop",
      "observe",
    ],
    connection,
    lease,
    arm: "follower",
  });
  const calls = [0.1, 0.2, 0.3].map((opening) => ({
    id: String(opening),
    name: "set_arm_gripper",
    arguments: { arm: "follower", opening },
  }));
  const pending = robot.execute(calls);
  await Promise.resolve();
  expect(started).toEqual([0.1, 0.2, 0.3]);
  releaseFirst?.();
  const results = await pending;
  expect(started).toEqual([0.1, 0.2, 0.3]);
  expect(results.map((item) => item.call.id)).toEqual(["0.1", "0.2", "0.3"]);
  expect(results[1]?.interrupted).toBe(true);
  expect(results[2]?.isError).toBe(true);
  robot.close();
});

test("single-arm pose/gripper specialization hides and rejects joint targets and standalone actions", async () => {
  const original = createRobotTools({
    connection,
    lease: fakeLease(),
    arm: "follower",
    descriptions: { preview_pose: "Preview poses before moving" },
  });
  const specialized = original
    .mapTools((capabilities) => capabilities.toReversed())
    .mapTools(filterSequenceActions((action) => action !== "joints"));
  expect(specialized.tools.map((tool) => tool.name)).toEqual([
    "observe",
    "stop",
    "preview_pose",
    "action_sequence",
  ]);
  expect(
    JSON.stringify(
      original.tools.find((tool) => tool.name === "action_sequence")
        ?.parameters,
    ),
  ).toContain("joints_degrees");
  expect(
    specialized.tools.find((tool) => tool.name === "preview_pose")?.description,
  ).toBe("Preview poses before moving");
  const schema = specialized.tools.find(
    (tool) => tool.name === "action_sequence",
  )?.parameters;
  expect(JSON.stringify(schema)).toContain("position_mm");
  expect(JSON.stringify(schema)).not.toContain("joints_degrees");
  expect(JSON.stringify(schema)).not.toContain('"arms"');
  const [hiddenJoint, standalone] = await specialized.execute([
    {
      id: "joint",
      name: "action_sequence",
      arguments: { steps: [{ goal: { joints_degrees: [0, 0, 0, 0, 0] } }] },
    },
    {
      id: "standalone",
      name: "set_arm_gripper",
      arguments: { arm: "follower", opening: 0.5 },
    },
  ]);
  expect(hiddenJoint?.isError).toBe(true);
  expect(hiddenJoint?.text).toContain("not available");
  expect(standalone?.isError).toBe(true);
  expect(standalone?.text).toContain("not available");
  specialized.close();
});

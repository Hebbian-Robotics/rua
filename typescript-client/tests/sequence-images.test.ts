import { createServer } from "node:http";
import { expect, test, vi } from "vitest";
import { renderOutput } from "../src/output-format.js";

test("observe exposes retained images only when history is requested", async () => {
  const { createRobotTools, createStationConnection, apiSchemas } =
    await import("../src/index.js");
  const { fakeLease, stateMessage } = await import("./station-fakes.js");
  const state = apiSchemas.shape.StationState.parse(stateMessage([{}]).state);
  const server = createServer((request, response) => {
    if (request.url === "/observations") {
      response.writeHead(200, { "Content-Type": "application/json" }).end(
        JSON.stringify({
          status: "observation_captured",
          state,
          cameras: {
            front: {
              jpeg_base64: "AQID",
              captured_at_unix_ns: null,
              retrieved_between_unix_seconds: [0, 0],
            },
          },
          missing_optional_cameras: [],
        }),
      );
      return;
    }
    if (request.url === "/action_sequence/sequence/images") {
      response.writeHead(200, { "Content-Type": "application/json" }).end(
        JSON.stringify({
          images: [
            {
              camera: "front",
              frame: {
                jpeg_base64: "AQID",
                captured_at_unix_ns: null,
                retrieved_between_unix_seconds: [0, 0],
              },
              elapsed_seconds: 0,
              boundary: "end",
            },
          ],
          expires_in_seconds: 300,
          missed_samples: 0,
          truncated: false,
        }),
      );
      return;
    }
    response.writeHead(404).end();
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const address = server.address();
  if (!address || typeof address === "string")
    throw new Error("Missing server address");
  const connection = createStationConnection(
    `http://127.0.0.1:${address.port}`,
    "agent",
  );
  const stateSpy = vi
    .spyOn(connection, "waitForState")
    .mockResolvedValue(state);
  const sequenceSpy = vi
    .spyOn(connection, "waitForActionSequence")
    .mockResolvedValue(
      apiSchemas.shape.ActionSequenceRecord.parse({
        id: "sequence",
        arms: ["follower"],
        phase: { state: "completed" },
        requested_steps: [],
        compilation_id: "compilation",
        continuation: null,
        steps: [],
      }),
    );
  const robot = createRobotTools({
    connection,
    lease: fakeLease({ startActionSequence: async () => "sequence" }),
    arm: "follower",
    cameras: ["front"],
  });
  try {
    const [missing] = await robot.execute([
      {
        id: "missing",
        name: "observe",
        arguments: { source: "last_sequence" },
      },
    ]);
    expect(missing?.isError).toBe(true);
    expect(Object.keys(missing?.cameras ?? {})).toEqual([]);
    const [motion] = await robot.execute([
      {
        id: "move",
        name: "action_sequence",
        arguments: { steps: [{ gripper_opening: 0.5 }] },
      },
    ]);
    expect(motion?.isError).toBe(false);
    const exposed = robot.sequenceImages()?.images[0]?.jpeg;
    expect(exposed).toEqual(new Uint8Array([1, 2, 3]));
    exposed?.fill(0);
    expect(robot.sequenceImages()?.images[0]?.jpeg).toEqual(
      new Uint8Array([1, 2, 3]),
    );
    expect(Object.keys(motion?.cameras ?? {})).toEqual([]);
    expect(Object.keys((await robot.observe()).cameras)).toEqual(["front"]);
    for (let attempt = 0; attempt < 2; attempt++) {
      const [review] = await robot.execute([
        {
          id: `review-${attempt}`,
          name: "observe",
          arguments: { source: "last_sequence" },
        },
      ]);
      expect(review?.isError).toBe(false);
      expect(review?.data).toMatchObject({
        source: "last_sequence",
        sequence_call_id: "move",
      });
      expect(Object.keys(review?.cameras ?? {}).length).toBeGreaterThan(0);
      expect(
        Object.keys(review?.cameras ?? {}).every((name) =>
          name.startsWith("Sequence move"),
        ),
      ).toBe(true);
      expect(Object.keys((await robot.observe()).cameras)).toEqual(["front"]);
    }
    const hiddenView = robot.withOutputFormatter((output) =>
      renderOutput(
        output.sequenceImages
          ? {
              ...output,
              sequenceImages: { ...output.sequenceImages, images: [] },
            }
          : output,
      ),
    );
    const [hidden] = await hiddenView.execute([
      { id: "hidden", name: "observe", arguments: { source: "last_sequence" } },
    ]);
    expect(hidden?.cameras).toEqual({});
    const [current] = await robot.execute([
      { id: "current", name: "observe", arguments: {} },
    ]);
    expect(Object.keys(current?.cameras ?? {})).toEqual(["front"]);
    const broken = robot.withOutputFormatter(() => {
      throw new Error("presentation failed");
    });
    await expect(
      broken.execute([{ id: "broken", name: "observe", arguments: {} }]),
    ).rejects.toThrow("presentation failed");
    const [invalid] = await robot.execute([
      { id: "invalid", name: "observe", arguments: { source: "everything" } },
    ]);
    expect(invalid?.isError).toBe(true);
  } finally {
    robot.close();
    stateSpy.mockRestore();
    sequenceSpy.mockRestore();
    server.closeAllConnections();
    await new Promise<void>((resolve, reject) =>
      server.close((error) => (error ? reject(error) : resolve())),
    );
  }
});

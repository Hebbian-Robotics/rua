import { createServer } from "node:http";
import { expect, test, vi } from "vitest";
import { renderOutput } from "../src/output-format.js";
import {
  captureSequenceImages,
  SequenceImages,
} from "../src/sequence-images.js";

test.each([1, 1024 * 1024])(
  "history bounds retained and model images (%i byte JPEGs)",
  (size) => {
    vi.useFakeTimers();
    const store = new SequenceImages();
    try {
      const history = store.begin("move");
      for (let index = 0; index < 100; index++) {
        store.add(history, {
          camera: "front",
          jpeg: new Uint8Array(size),
          retrievedBetweenUnixSeconds: [100 + index, 100.1 + index],
          elapsedSeconds: index,
          boundary: index === 99 ? "end" : "motion",
        });
      }
      vi.advanceTimersByTime(300_001);
      expect(store.snapshot()).toBeDefined();
      store.finish(history);
      expect(history.images.length).toBeLessThanOrEqual(64);
      expect(
        history.images.reduce((sum, image) => sum + image.jpeg.byteLength, 0),
      ).toBeLessThanOrEqual(16 * 1024 * 1024);
      const selected = store.snapshot();
      expect(selected?.truncated).toBe(true);
      expect(selected?.images.length).toBeLessThanOrEqual(8);
      expect(
        selected?.images.reduce((sum, image) => sum + image.jpeg.byteLength, 0),
      ).toBeLessThanOrEqual(2 * 1024 * 1024);
      expect(selected?.images.at(-1)?.boundary).toBe("end");
      vi.advanceTimersByTime(300_001);
      expect(store.snapshot()).toBeUndefined();
      expect(history.images).toEqual([]);
    } finally {
      store.clear();
      vi.useRealTimers();
    }
  },
);

test("sequence sampling does not wait for an unavailable camera and retains another camera", async () => {
  let motionStarted = false;
  let stalledFirstRequest = false;
  let reportClosed: ((motionHadStarted: boolean) => void) | undefined;
  const stalledRequestClosed = new Promise<boolean>((resolve) => {
    reportClosed = resolve;
  });
  const server = createServer((request, response) => {
    if (request.url === "/cameras/slow") {
      if (stalledFirstRequest) {
        response.writeHead(503).end();
      } else {
        stalledFirstRequest = true;
        const motionHadStarted = motionStarted;
        response.once("close", () => reportClosed?.(motionHadStarted));
      }
      return;
    }
    response.writeHead(200, { "content-type": "image/jpeg" });
    response.end(new Uint8Array([1, 2, 3]));
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const address = server.address();
  if (!address || typeof address === "string")
    throw new Error("Missing server address");
  const store = new SequenceImages();
  let unsubscribed = false;
  try {
    const outcome = await captureSequenceImages(
      store,
      "move",
      ["slow", "front"],
      `http://127.0.0.1:${address.port}`,
      () => () => {
        unsubscribed = true;
      },
      async () => {
        motionStarted = true;
        expect(await stalledRequestClosed).toBe(true);
        return "completed";
      },
    );
    expect(outcome).toBe("completed");
    expect(unsubscribed).toBe(true);
    const history = store.snapshot();
    expect(history?.images.some((image) => image.camera === "front")).toBe(
      true,
    );
    expect(history?.missedSamples).toBeGreaterThan(0);
  } finally {
    store.clear();
    server.closeAllConnections();
    await new Promise<void>((resolve, reject) =>
      server.close((error) => (error ? reject(error) : resolve())),
    );
  }
});

test("observe exposes retained images only when history is requested", async () => {
  const { createRobotTools, createStationConnection, apiSchemas } =
    await import("../src/index.js");
  const { fakeLease, stateMessage } = await import("./station-fakes.js");
  const server = createServer((_request, response) => {
    response.end(new Uint8Array([1, 2, 3]));
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const address = server.address();
  if (!address || typeof address === "string")
    throw new Error("Missing server address");
  const connection = createStationConnection(
    `http://127.0.0.1:${address.port}`,
    "agent",
  );
  const state = apiSchemas.shape.StationState.parse(stateMessage([{}]).state);
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

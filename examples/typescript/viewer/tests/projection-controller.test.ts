import type { PreviewAnswer, PreviewPose } from "@hebbian/robot-use-client";
import { expect, test } from "vitest";
import {
  createProjectionController,
  interpretProjection,
} from "../src/features/control/projection-controller.js";

function projected(joint: number): PreviewAnswer {
  return {
    status: "projected",
    projection: {
      requested: null,
      jaw_yaw_ranges: [],
      calculation_milliseconds: 1,
      outcome: {
        kind: "projected",
        within_tolerance: true,
        reason: null,
        pose: {
          joints: [joint],
          position: [0, 0, 0],
          quaternion: [0, 0, 0, 1],
          orientation: { approach: [0, 0, -1], jaw_yaw_degrees: 0 },
          position_error_meters: 0,
          orientation_error_radians: null,
        },
      },
    },
  };
}

test("a cleared target never restores or authorizes an in-flight preview", async () => {
  let finish: ((answer: PreviewAnswer) => void) | undefined;
  let target: number[] | null = [0.1, 0, 0.2];
  const solutions: number[] = [];
  let clears = 0;
  const controller = createProjectionController({
    currentRequest: () =>
      target
        ? { arm: "follower", position: target, pose: { position: target } }
        : null,
    preview: () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
    onPending: () => {},
    onCalculating: () => {},
    onClear: () => {
      clears += 1;
    },
    onFailure: () => {},
    onSolution: (_solved, confirm) => {
      if (confirm) solutions.push(1);
    },
  });
  controller.targetChanged();
  await flushMicrotasks();
  expect(finish).toBeDefined();
  target = null;
  controller.clear();
  finish?.(projected(0.1));
  await flushMicrotasks();
  expect(clears).toBe(1);
  expect(solutions).toEqual([]);
  controller.destroy();
});

test("a changed target can show an older result but only the latest result is confirmed", async () => {
  let target = [0.1, 0, 0.2];
  let orientation: PreviewPose["orientation"] = null;
  const requests: PreviewPose[] = [];
  const replies: Array<(answer: PreviewAnswer) => void> = [];
  const results: Array<{ position: number[]; confirmed: boolean }> = [];
  const controller = createProjectionController({
    currentRequest: () => ({
      arm: "follower",
      position: target,
      pose: orientation
        ? { position: target, orientation }
        : { position: target },
    }),
    preview: (_arm, pose) => {
      requests.push(pose);
      return new Promise((resolve) => {
        replies.push(resolve);
      });
    },
    onPending: () => {},
    onCalculating: () => {},
    onClear: () => {},
    onFailure: () => {},
    onSolution: (solved, confirm) =>
      results.push({ position: solved.requestedPosition, confirmed: confirm }),
  });
  controller.targetChanged();
  await flushMicrotasks();
  target = [0.2, 0, 0.2];
  orientation = { kind: "grasp", approach: "forward", jaw_yaw_degrees: 90 };
  controller.targetChanged();
  replies[0]?.(projected(0.1));
  await flushMicrotasks();
  expect(requests).toHaveLength(2);
  expect(requests[1]).toEqual({ position: [0.2, 0, 0.2], orientation });
  replies[1]?.(projected(0.2));
  await flushMicrotasks();
  expect(results).toEqual([
    { position: [0.1, 0, 0.2], confirmed: false },
    { position: [0.2, 0, 0.2], confirmed: true },
  ]);
  controller.destroy();
});

test("an approximate projection preserves the requested pose and exact feasible joints", () => {
  const answer = projected(0.5);
  if (
    answer.status !== "projected" ||
    answer.projection.outcome.kind !== "projected"
  )
    throw new Error("invalid fixture");
  const projection = {
    ...answer.projection,
    requested: {
      orientation: { approach: [1, 0, 0], jaw_yaw_degrees: 90 },
      quaternion: [0, 0, 0, 1],
    },
    jaw_yaw_ranges: [
      { from_degrees: 0, to_degrees: 20, reachable: false, reason: "Limit" },
    ],
    outcome: {
      kind: "projected" as const,
      within_tolerance: false,
      pose: { ...answer.projection.outcome.pose, position_error_meters: 0.02 },
      reason: "Limited to a feasible pose",
    },
  };
  const result = interpretProjection(
    {
      arm: "follower",
      requestPose: { position: [0.2, 0, 0.1] },
      projection,
      requestedPosition: [0.2, 0, 0.1],
    },
    true,
  );
  expect(result.kind).toBe("available");
  if (result.kind !== "available")
    throw new Error("expected available preview");
  expect(result.confirmedPose).toMatchObject({
    joints: [0.5],
    requestedPosition: [0.2, 0, 0.1],
    requestedQuaternion: [0, 0, 0, 1],
  });
  expect(result.chosenOrientation?.jaw_yaw_degrees).toBe(90);
  expect(result.jawYawRanges[0]?.reachable).toBe(false);
  expect(result.withinTolerance).toBe(false);
  expect(result.message).toContain("approximate pose");
  expect(
    interpretProjection(
      {
        arm: "follower",
        requestPose: { position: [0.2, 0, 0.1] },
        projection,
        requestedPosition: [0.2, 0, 0.1],
      },
      false,
    ),
  ).toMatchObject({ kind: "available", confirmedPose: null });
});

test("an unavailable preview cannot produce a move", () => {
  const answer = projected(0.5);
  if (answer.status !== "projected") throw new Error("invalid fixture");
  const projection = {
    ...answer.projection,
    outcome: { kind: "unavailable" as const, reason: "Invalid starting state" },
  };
  expect(
    interpretProjection(
      {
        arm: "follower",
        requestPose: { position: [1, 2, 3] },
        projection,
        requestedPosition: [1, 2, 3],
      },
      true,
    ),
  ).toEqual({
    kind: "unavailable",
    reason: "Invalid starting state",
    chosenOrientation: null,
    jawYawRanges: [],
  });
});

async function flushMicrotasks(): Promise<void> {
  for (let turn = 0; turn < 10; turn += 1) await Promise.resolve();
}

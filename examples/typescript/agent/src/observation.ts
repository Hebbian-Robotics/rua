// What the model sees each turn, traced and recorded for the viewer.

import {
  apiSchemas,
  type RobotObservation,
  type RobotToolResult,
} from "hebbian-rua";
import { type RunEnvironment, reportEvent } from "./environment.js";
import type { Observation } from "./providers/types.js";
import { MEMORY_TOOLS } from "./tools.js";
import { KIND_ATTRIBUTE } from "./tracing.js";

export interface RevisedObservation {
  observation: Observation;
  revisions: Record<string, number>;
}

export function cameraNames(cameras: string | undefined): string[] {
  if (cameras === undefined) return [];
  const names = cameras
    .split(",")
    .map((name) => name.trim())
    .filter((name) => name !== "");
  if (new Set(names).size !== names.length)
    throw new Error(`Camera names repeat: ${cameras}`);
  return names;
}

export function reuseBatchObservation(results: RobotToolResult[]): {
  captured?: RobotObservation;
  results: RobotToolResult[];
} {
  let observationIndex: number | undefined;
  for (const [index, result] of results.entries()) {
    const name = result.call.name;
    if (name === "preview_pose" || MEMORY_TOOLS.includes(name)) continue;
    if (name !== "observe") return { results };
    if (result.isError || result.sequenceImages) continue;
    if (observationIndex !== undefined) return { results };
    observationIndex = index;
  }
  const observation =
    observationIndex === undefined ? undefined : results[observationIndex];
  if (observation === undefined) return { results };
  const parsed = apiSchemas.shape.StationState.safeParse(observation.data);
  if (!parsed.success) return { results };

  return {
    captured: {
      data: parsed.data,
      text: observation.text,
      cameras: observation.cameras ?? {},
      revisions: Object.fromEntries(
        parsed.data.arms.map((arm) => [arm.arm, arm.revision]),
      ),
    },
    results: results.map((result, index) =>
      index === observationIndex
        ? { ...result, text: "ok", data: "ok", cameras: {} }
        : result,
    ),
  };
}

export async function observe(
  environment: RunEnvironment,
  note: string,
  alreadyCaptured?: RobotObservation,
): Promise<RevisedObservation> {
  const { robot, withSpan, memory } = environment;
  const captured = await withSpan(
    "observe",
    {
      [KIND_ATTRIBUTE]: "observe",
      "hebbian.arm": environment.arm,
      "hebbian.cameras": environment.cameras.join(","),
      "hebbian.observation.note": note,
      "hebbian.memory": memory.viewerSnapshot(),
    },
    () =>
      alreadyCaptured
        ? Promise.resolve({
            ...alreadyCaptured,
            text: [
              ...memory.observationLines(),
              note,
              alreadyCaptured.text,
            ].join("\n"),
          })
        : robot.observeWhenReady({
            note,
            extraLines: memory.observationLines(),
          }),
  );
  reportEvent(environment.connection, {
    type: "observation",
    text: captured.text,
    cameras: Object.fromEntries(
      Object.entries(captured.cameras).map(([name, jpeg]) => [
        name,
        Buffer.from(jpeg).toString("base64"),
      ]),
    ),
  });
  return {
    observation: { text: captured.text, cameras: captured.cameras },
    revisions: captured.revisions,
  };
}

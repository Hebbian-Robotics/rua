// What the model sees each turn, traced and recorded for the viewer.

import type { RobotObservation } from "hebbian-rua";
import { type RunEnvironment, reportEvent } from "./environment.js";
import type { Observation } from "./providers/types.js";
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

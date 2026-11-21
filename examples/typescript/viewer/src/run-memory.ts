import * as z from "zod";
import { type LiveRun, liveRunSchema } from "./generated/live-run-schema.js";
import type { RecordingTimeline } from "./recording-timeline.js";

type RunMemory = LiveRun["memory"];
const memorySchema = liveRunSchema.shape.memory;
const memoryEventSchema = z.object({
  type: z.enum(["run_started", "plan_update", "notes_update"]),
  task: z.string().optional(),
  steps: memorySchema.shape.plan.optional(),
  notes: memorySchema.shape.notes.optional(),
  timestamp_unix_ns: z.string().regex(/^\d+$/).optional(),
});
const recordedEventSchema = z.object({ event: memoryEventSchema });
const emptyMemory = (): RunMemory => ({
  task: null,
  plan: null,
  notes: [],
});

export function recordedMemory(
  timeline: RecordingTimeline,
): { timestamp: number; memory: RunMemory }[] {
  const updates = timeline.events
    .filter((event) => event.topic === "/model/events")
    .flatMap((event) => {
      const parsed = recordedEventSchema.safeParse(event.data);
      if (!parsed.success) return [];
      return [
        {
          timestamp: parsed.data.event.timestamp_unix_ns
            ? Number(parsed.data.event.timestamp_unix_ns) / 1e9
            : event.timestamp,
          event: parsed.data.event,
        },
      ];
    })
    .sort((first, second) => first.timestamp - second.timestamp);
  let memory = emptyMemory();
  return updates.map(({ timestamp, event }) => {
    if (event.type === "run_started")
      memory = { ...emptyMemory(), task: event.task ?? null };
    else if (event.type === "plan_update" && event.steps !== undefined)
      memory = { ...memory, plan: event.steps };
    else if (event.type === "notes_update" && event.notes !== undefined)
      memory = { ...memory, notes: event.notes };
    return { timestamp, memory };
  });
}

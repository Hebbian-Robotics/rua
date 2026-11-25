import { apiSchemas, type LiveRun } from "hebbian-rua";
import * as z from "zod";
import type { RecordingTimeline } from "./recording-timeline.js";

type RunMemory = LiveRun["memory"];
type RunUsage = NonNullable<LiveRun["usage"]>;

const recordedEventSchema = z.object({
  event: z.intersection(
    apiSchemas.shape.KnownRunEvent,
    z.object({
      timestamp_unix_ns: apiSchemas.shape.UnixNanos.optional(),
    }),
  ),
});

function emptyMemory(): RunMemory {
  return { task: null, plan: null, notes: [] };
}

export function recordedRunTelemetry(timeline: RecordingTimeline) {
  const updates = timeline.events
    .flatMap(({ topic, data, timestamp }) => {
      if (topic !== "/model/events") return [];
      const parsed = recordedEventSchema.safeParse(data);
      if (!parsed.success) return [];
      const event = parsed.data.event;
      return [
        {
          timestamp: event.timestamp_unix_ns
            ? Number(event.timestamp_unix_ns) / 1e9
            : timestamp,
          event,
        },
      ];
    })
    .sort((first, second) => first.timestamp - second.timestamp);

  const memoryUpdates: { timestamp: number; memory: RunMemory }[] = [];
  const usageUpdates: { timestamp: number; usage: RunUsage }[] = [];
  const latestByRun = new Map<string, RunUsage>();
  let memory = emptyMemory();
  let total: RunUsage = {
    run_id: "",
    turns: 0,
    input_tokens: 0,
    cached_input_tokens: 0,
    output_tokens: 0,
    reasoning_tokens: 0,
  };
  for (const { timestamp, event } of updates) {
    switch (event.type) {
      case "run_started":
        memory = { ...emptyMemory(), task: event.task };
        break;
      case "plan_update":
        memory = { ...memory, plan: event.steps };
        break;
      case "notes_update":
        memory = { ...memory, notes: event.notes };
        break;
      case "usage_update": {
        // CONSTRAINT: Each run reports cumulative counts. Add only the change in that run
        // so replacing a snapshot or starting a continuation keeps the total.
        const usage = event.usage;
        const previous = latestByRun.get(usage.run_id);
        total = {
          run_id: usage.run_id,
          turns: total.turns + usage.turns - (previous?.turns ?? 0),
          input_tokens:
            total.input_tokens +
            usage.input_tokens -
            (previous?.input_tokens ?? 0),
          cached_input_tokens:
            total.cached_input_tokens +
            usage.cached_input_tokens -
            (previous?.cached_input_tokens ?? 0),
          output_tokens:
            total.output_tokens +
            usage.output_tokens -
            (previous?.output_tokens ?? 0),
          reasoning_tokens:
            total.reasoning_tokens +
            usage.reasoning_tokens -
            (previous?.reasoning_tokens ?? 0),
        };
        latestByRun.set(usage.run_id, usage);
        usageUpdates.push({ timestamp, usage: total });
        continue;
      }
    }
    memoryUpdates.push({ timestamp, memory });
  }
  return { memoryUpdates, usageUpdates };
}

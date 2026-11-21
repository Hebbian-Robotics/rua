import * as z from "zod";
import { type RunUsage, runUsageSchema } from "./generated/run-usage-schema.js";
import type { RecordingTimeline } from "./recording-timeline.js";

const usageEventSchema = z.object({
  type: z.literal("usage_update"),
  usage: runUsageSchema,
});

export function recordedUsage(
  timeline: RecordingTimeline,
): { timestamp: number; usage: RunUsage }[] {
  const updates = timeline.events
    .flatMap((event) => {
      const parsed = usageEventSchema.safeParse(event.data);
      return parsed.success
        ? [{ timestamp: event.timestamp, usage: parsed.data.usage }]
        : [];
    })
    .sort((first, second) => first.timestamp - second.timestamp);
  // Each run reports cumulative snapshots. Replace that run's latest snapshot,
  // then total the runs so a continuation never resets the recording's counter.
  const latestByRun = new Map<string, RunUsage>();
  return updates.map((update) => {
    latestByRun.set(update.usage.runId, update.usage);
    const total: RunUsage = {
      runId: update.usage.runId,
      turns: 0,
      inputTokens: 0,
      cachedInputTokens: 0,
      outputTokens: 0,
      reasoningTokens: 0,
    };
    for (const usage of latestByRun.values()) {
      total.turns += usage.turns;
      total.inputTokens += usage.inputTokens;
      total.cachedInputTokens += usage.cachedInputTokens;
      total.outputTokens += usage.outputTokens;
      total.reasoningTokens += usage.reasoningTokens;
    }
    return { timestamp: update.timestamp, usage: total };
  });
}

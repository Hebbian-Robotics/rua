import type { Run } from "../shared/run-api.js";
import type { LiveRun } from "./generated/live-run-schema.js";
import type { TraceSpan } from "./span-time.js";

type RunContents = {
  agentRun: Run | null;
  spans: TraceSpan[];
  usage: LiveRun["usage"];
  memory: LiveRun["memory"] | null;
};

// INVARIANT: Clearing dismisses the current run, including later updates to its traces.
export type ClearedRun = {
  agentRunId: string | null;
  usageRunId: string | null;
  traceIds: Set<string>;
};

export function clearRunContents(contents: RunContents): ClearedRun {
  return {
    agentRunId: contents.agentRun?.id ?? null,
    usageRunId: contents.usage?.runId ?? null,
    traceIds: new Set(contents.spans.map((span) => span.traceId)),
  };
}

export function visibleRunContents(
  contents: RunContents,
  cleared: ClearedRun | null,
): RunContents {
  if (!cleared) return contents;
  const agentRun =
    contents.agentRun?.id === cleared.agentRunId ? null : contents.agentRun;
  const usage =
    contents.usage?.runId === cleared.usageRunId ? null : contents.usage;
  return {
    agentRun,
    usage,
    memory: (contents.usage ? usage : agentRun) ? contents.memory : null,
    spans: contents.spans.filter((span) => !cleared.traceIds.has(span.traceId)),
  };
}

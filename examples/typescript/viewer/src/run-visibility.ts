import type { LiveRun } from "hebbian-rua";
import type { Run } from "../shared/run-api.js";
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
    usageRunId: contents.usage?.run_id ?? null,
    traceIds: new Set(contents.spans.map((span) => span.trace_id)),
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
    contents.usage?.run_id === cleared.usageRunId ? null : contents.usage;
  return {
    agentRun,
    usage,
    memory: (contents.usage ? usage : agentRun) ? contents.memory : null,
    spans: contents.spans.filter(
      (span) => !cleared.traceIds.has(span.trace_id),
    ),
  };
}

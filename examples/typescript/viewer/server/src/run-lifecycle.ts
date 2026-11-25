import { assign, type SnapshotFrom, setup } from "xstate";
import type { AgentOutcome } from "../../../agent/src/index.js";
import type { RunPhase } from "../../shared/run-api.js";

type EndedPhase = Extract<RunPhase, { state: "ended" }>;

export const runLifecycle = setup({
  types: {} as {
    input: { phase: RunPhase };
    context: { phase: RunPhase };
    events:
      | { type: "STOP" }
      | { type: "SHUTDOWN" }
      | { type: "END"; phase: EndedPhase };
  },
  guards: { alreadyEnded: ({ context }) => context.phase.state === "ended" },
  actions: {
    markStopping: assign({
      phase: ({ context }) =>
        context.phase.state === "running"
          ? { state: "stopping" as const, startedAt: context.phase.startedAt }
          : context.phase,
    }),
    markEnded: assign({
      phase: ({ event, context }) =>
        event.type === "END" ? event.phase : context.phase,
    }),
  },
}).createMachine({
  id: "agent-run",
  initial: "restore",
  context: ({ input }) => ({ phase: input.phase }),
  states: {
    restore: {
      always: [
        { guard: "alreadyEnded", target: "ended" },
        { target: "running" },
      ],
    },
    running: {
      on: {
        STOP: { target: "stopping", actions: "markStopping" },
        SHUTDOWN: "interrupting",
        END: { target: "ended", actions: "markEnded" },
      },
    },
    stopping: {
      on: {
        SHUTDOWN: "interrupting",
        END: { target: "ended", actions: "markEnded" },
      },
    },
    interrupting: { on: { END: { target: "ended", actions: "markEnded" } } },
    ended: { type: "final" },
  },
});

export type TerminalRunOutcome = Pick<EndedPhase, "outcome" | "response">;

export function terminalRunOutcome(
  lifecycleState: SnapshotFrom<typeof runLifecycle>["value"],
  agentOutcome: AgentOutcome,
): TerminalRunOutcome {
  if (lifecycleState === "interrupting")
    return { outcome: { kind: "interrupted" } };
  if (lifecycleState === "stopping") return { outcome: { kind: "stopped" } };
  switch (agentOutcome.kind) {
    case "completed":
      return { outcome: { kind: "completed" }, response: agentOutcome.result };
    case "stopped":
      return { outcome: { kind: "stopped" } };
    case "incomplete":
      return { outcome: { kind: "incomplete", detail: agentOutcome.result } };
    case "failed":
      return { outcome: { kind: "failed", detail: agentOutcome.reason } };
  }
}

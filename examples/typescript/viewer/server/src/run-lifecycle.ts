import { assign, setup } from "xstate";
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

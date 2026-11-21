import { assign, setup } from "xstate";

export function createCameraConnectionMachine(connect: () => Promise<void>) {
  return setup({
    types: {} as {
      context: { failuresInARow: number };
      events:
        | { type: "CONNECTED" }
        | { type: "DISCONNECTED" }
        | { type: "FAILED" };
    },
    delays: {
      retryDelay: ({ context }) =>
        context.failuresInARow === 1
          ? 0
          : Math.min(30000, 250 * 2 ** (context.failuresInARow - 2)),
    },
    actions: {
      connect: () => void connect(),
      noteFailure: assign({
        failuresInARow: ({ context }) => context.failuresInARow + 1,
      }),
      resetFailures: assign({ failuresInARow: 0 }),
    },
  }).createMachine({
    initial: "connecting",
    context: { failuresInARow: 0 },
    states: {
      connecting: {
        entry: "connect",
        on: {
          CONNECTED: { target: "connected", actions: "resetFailures" },
          FAILED: { target: "retrying", actions: "noteFailure" },
        },
      },
      connected: {
        on: { DISCONNECTED: { target: "retrying", actions: "noteFailure" } },
      },
      retrying: { after: { retryDelay: "connecting" } },
    },
  });
}

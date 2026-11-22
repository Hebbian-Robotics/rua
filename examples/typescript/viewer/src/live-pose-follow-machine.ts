import { setup } from "xstate";

// INVARIANT: Arming follow and holding the pointer are independent operator actions.
export function createLivePoseFollowMachine(stopMotion: () => void) {
  return setup({
    types: {} as {
      events:
        | { type: "ENABLE" }
        | { type: "DISABLE" }
        | { type: "DRAG_START" }
        | { type: "DRAG_END" }
        | { type: "STOP" };
    },
    actions: { stopMotion },
  }).createMachine({
    id: "live-pose-follow",
    type: "parallel",
    on: { STOP: { actions: "stopMotion" } },
    states: {
      control: {
        initial: "disarmed",
        states: {
          disarmed: {
            on: { ENABLE: "armed", DISABLE: { actions: "stopMotion" } },
          },
          armed: {
            on: { DISABLE: { target: "disarmed", actions: "stopMotion" } },
          },
        },
      },
      pointer: {
        initial: "released",
        states: {
          released: {
            on: { DRAG_START: "dragging", DRAG_END: { actions: "stopMotion" } },
          },
          dragging: {
            on: { DRAG_END: { target: "released", actions: "stopMotion" } },
          },
        },
      },
    },
  });
}

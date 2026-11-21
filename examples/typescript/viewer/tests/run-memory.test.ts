import { expect, test } from "vitest";
import { recordedMemory } from "../src/run-memory.js";

test("restores memory from recorded run event envelopes", () => {
  const updates = recordedMemory({
    clips: [],
    events: [
      {
        timestamp: 1,
        topic: "/model/events",
        data: {
          seq: 1,
          received_unix_nano: "1000000000",
          event: { type: "run_started", task: "Pick up the block" },
        },
      },
      {
        timestamp: 2,
        topic: "/model/events",
        data: {
          seq: 2,
          received_unix_nano: "2000000000",
          event: { type: "notes_update", notes: ["Block located"] },
        },
      },
    ],
    truncated: false,
    state: null,
    spans: [],
  });

  expect(updates.map(({ memory }) => memory)).toEqual([
    { task: "Pick up the block", plan: null, notes: [] },
    { task: "Pick up the block", plan: null, notes: ["Block located"] },
  ]);
});

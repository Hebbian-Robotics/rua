import { expect, test } from "vitest";
import { recordedRunTelemetry } from "../src/recorded-run.js";

test("restores memory from recorded run event envelopes", () => {
  const { memoryUpdates } = recordedRunTelemetry({
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

  expect(memoryUpdates.map(({ memory }) => memory)).toEqual([
    { task: "Pick up the block", plan: null, notes: [] },
    { task: "Pick up the block", plan: null, notes: ["Block located"] },
  ]);
});

test("replays cumulative token usage across recorded runs", () => {
  const { usageUpdates } = recordedRunTelemetry({
    clips: [],
    events: [
      { timestamp: 1, runId: "first", turns: 1, inputTokens: 42 },
      { timestamp: 2, runId: "first", turns: 2, inputTokens: 60 },
      { timestamp: 3, runId: "continuation", turns: 1, inputTokens: 20 },
    ].map(({ timestamp, runId, turns, inputTokens }) => ({
      timestamp,
      topic: "/model/events",
      data: {
        seq: timestamp,
        received_unix_nano: String(timestamp * 1e9),
        event: {
          type: "usage_update",
          usage: {
            run_id: runId,
            turns,
            input_tokens: inputTokens,
            cached_input_tokens: 0,
            output_tokens: 0,
            reasoning_tokens: 0,
          },
        },
      },
    })),
    truncated: false,
    state: null,
    spans: [],
  });

  expect(
    usageUpdates.map(({ timestamp, usage }) => [
      timestamp,
      usage.turns,
      usage.input_tokens,
    ]),
  ).toEqual([
    [1, 1, 42],
    [2, 2, 60],
    [3, 3, 80],
  ]);
});

import { expect, test } from "vitest";
import { planToolBatch } from "../src/execution.js";
import { wireTurn } from "../src/loop.js";

function calls(...names: string[]) {
  return names.map((name, index) => ({ id: `c${index}`, name, arguments: {} }));
}

test("plan and notes updates come first in call order, then the robot's calls together", () => {
  const batch = calls("action_sequence", "update_notes", "stop", "update_plan");
  expect(planToolBatch(batch)).toEqual({
    kind: "run",
    memoryCalls: [
      { index: 1, call: batch[1] },
      { index: 3, call: batch[3] },
    ],
    finishCalls: [],
    robotCalls: [
      { index: 0, call: batch[0] },
      { index: 2, call: batch[2] },
    ],
  });
});

test("model turns are recorded in the agent's wire shape", () => {
  const usage = {
    model: "m",
    inputTokens: 3,
    cachedInputTokens: 1,
    outputTokens: 2,
    reasoningTokens: 1,
    cacheWriteTokens: 0,
  };
  expect(
    wireTurn({
      kind: "turn",
      toolCalls: [{ id: "c", name: "stop", arguments: {} }],
      reasoningSummaries: ["s"],
      usage,
    }),
  ).toEqual({
    tool_calls: [{ call_id: "c", name: "stop", arguments: {} }],
    reasoning_summaries: ["s"],
    usage: {
      model: "m",
      input_tokens: 3,
      cached_input_tokens: 1,
      output_tokens: 2,
      reasoning_tokens: 1,
      cache_write_tokens: 0,
    },
  });
  expect(wireTurn({ kind: "refusal", usage: null })).toEqual({ usage: null });
});

import { expect, test } from "vitest";
import { planToolBatch } from "../src/execution.js";
import { wireTurn } from "../src/loop.js";

function calls(...names: string[]) {
  return names.map((name, index) => ({ id: `c${index}`, name, arguments: {} }));
}

test("plan and notes updates come first in call order, then the robot's calls together", () => {
  expect(
    planToolBatch(
      calls("action_sequence", "update_notes", "stop", "update_plan"),
    ),
  ).toEqual({
    kind: "run",
    memoryIndexes: [1, 3],
    finishIndexes: [],
    robotIndexes: [0, 2],
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

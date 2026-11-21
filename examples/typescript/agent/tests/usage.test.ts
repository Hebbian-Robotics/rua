import { expect, test } from "vitest";
import { estimateModelCost } from "../src/usage.js";

test("estimates token costs with cached reads and writes billed separately", async () => {
  const cost = await estimateModelCost("anthropic", {
    model: "claude-fable-5-1",
    inputTokens: 1_000_000,
    cachedInputTokens: 200_000,
    cacheWriteTokens: 100_000,
    outputTokens: 100_000,
    reasoningTokens: 20_000,
  });
  expect(cost).toBeCloseTo(13.3);
});

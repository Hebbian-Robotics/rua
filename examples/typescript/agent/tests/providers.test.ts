import type { BetaMessage } from "@anthropic-ai/sdk/resources/beta/messages";
import type { Response } from "openai/resources/responses/responses";
import { expect, test } from "vitest";
import { claudeTurn, claudeUsage } from "../src/providers/claude.js";
import {
  GeminiInteractionError,
  isProviderError,
  OpenAIResponseError,
} from "../src/providers/errors.js";
import {
  type GeminiInteraction,
  geminiTurn,
  geminiUsage,
} from "../src/providers/gemini.js";
import { openaiTurn, openaiUsage } from "../src/providers/openai.js";

function claudeMessage(overrides: Record<string, unknown>): BetaMessage {
  return {
    model: "claude-answered",
    stop_reason: "tool_use",
    usage: {
      input_tokens: 10,
      output_tokens: 7,
      cache_read_input_tokens: 5,
      cache_creation_input_tokens: 3,
    },
    content: [],
    ...overrides,
  } as unknown as BetaMessage;
}

test("Claude input tokens include cache reads and writes", () => {
  expect(claudeUsage(claudeMessage({}))).toEqual({
    model: "claude-answered",
    inputTokens: 18,
    cachedInputTokens: 5,
    outputTokens: 7,
    reasoningTokens: 0,
    cacheWriteTokens: 3,
  });
  const bare = claudeMessage({
    usage: {
      input_tokens: 1,
      output_tokens: 2,
      cache_read_input_tokens: null,
      cache_creation_input_tokens: null,
    },
  });
  expect(claudeUsage(bare).inputTokens).toBe(1);
});

test("a Claude reply becomes tool calls and thinking summaries, and a refusal has none", () => {
  const turn = claudeTurn(
    claudeMessage({
      content: [
        { type: "thinking", thinking: "plan it", signature: "s" },
        { type: "thinking", thinking: "", signature: "s" },
        { type: "text", text: "ok" },
        { type: "tool_use", id: "t1", name: "stop", input: {} },
      ],
    }),
  );
  expect(turn).toMatchObject({
    kind: "turn",
    toolCalls: [{ id: "t1", name: "stop", arguments: {} }],
    reasoningSummaries: ["plan it"],
  });
  expect(claudeTurn(claudeMessage({ stop_reason: "refusal" })).kind).toBe(
    "refusal",
  );
});

function openaiResponse(overrides: Record<string, unknown>): Response {
  return {
    id: "resp_1",
    model: "gpt-answered",
    status: "completed",
    usage: {
      input_tokens: 100,
      input_tokens_details: { cached_tokens: 40 },
      output_tokens: 30,
      output_tokens_details: { reasoning_tokens: 12 },
    },
    output: [],
    ...overrides,
  } as unknown as Response;
}

test("OpenAI reasoning tokens come from the output token details", () => {
  expect(openaiUsage(openaiResponse({}))).toEqual({
    model: "gpt-answered",
    inputTokens: 100,
    cachedInputTokens: 40,
    outputTokens: 30,
    reasoningTokens: 12,
    cacheWriteTokens: 0,
  });
  expect(openaiUsage(openaiResponse({ usage: undefined }))).toBeNull();
});

test("an OpenAI response becomes calls, summaries or a refusal; a failed one throws", () => {
  const turn = openaiTurn(
    openaiResponse({
      output: [
        {
          type: "reasoning",
          summary: [{ type: "summary_text", text: "thought" }],
        },
        { type: "function_call", call_id: "c1", name: "stop", arguments: "{}" },
        {
          type: "function_call",
          call_id: "c2",
          name: "stop",
          arguments: "{oops",
        },
      ],
    }),
  );
  expect(turn).toMatchObject({
    kind: "turn",
    toolCalls: [
      { id: "c1", name: "stop", arguments: {} },
      { id: "c2", name: "stop", arguments: "{oops" },
    ],
    reasoningSummaries: ["thought"],
  });
  const refusal = openaiResponse({
    output: [
      { type: "message", content: [{ type: "refusal", refusal: "no" }] },
    ],
  });
  expect(openaiTurn(refusal).kind).toBe("refusal");
  expect(() =>
    openaiTurn(
      openaiResponse({
        status: "incomplete",
        incomplete_details: { reason: "max_output_tokens" },
      }),
    ),
  ).toThrow(OpenAIResponseError);
});

test("Gemini output tokens add the thought tokens, which are the reasoning tokens", () => {
  const interaction: GeminiInteraction = {
    id: "i1",
    status: "requires_action",
    model: "gemini-answered",
    usage: {
      total_input_tokens: 50,
      total_cached_tokens: 20,
      total_output_tokens: 9,
      total_thought_tokens: 6,
    },
    steps: [
      {
        type: "thought",
        summary: [{ type: "text", text: "hmm" }, { type: "image" }],
      },
      { type: "function_call", id: "g1", name: "stop", arguments: {} },
    ],
  };
  expect(geminiUsage("fallback", interaction)).toEqual({
    model: "gemini-answered",
    inputTokens: 50,
    cachedInputTokens: 20,
    outputTokens: 15,
    reasoningTokens: 6,
    cacheWriteTokens: 0,
  });
  expect(geminiTurn("fallback", interaction)).toMatchObject({
    toolCalls: [{ id: "g1", name: "stop", arguments: {} }],
    reasoningSummaries: ["hmm"],
  });
  expect(() => geminiTurn("m", { id: "i2", status: "failed" })).toThrow(
    GeminiInteractionError,
  );
});

test("provider failures are recognised so the loop can stop the arm", () => {
  expect(isProviderError(new OpenAIResponseError("x"))).toBe(true);
  expect(isProviderError(new GeminiInteractionError("x"))).toBe(true);
  expect(isProviderError(new Error("x"))).toBe(false);
});

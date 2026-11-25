import type Anthropic from "@anthropic-ai/sdk";
import type { GoogleGenAI } from "@google/genai";
import type OpenAI from "openai";
import { expect, test } from "vitest";
import { ClaudeConversation } from "../src/providers/claude.js";
import { GeminiConversation } from "../src/providers/gemini.js";
import { OpenAIConversation } from "../src/providers/openai.js";
import {
  type ConversationSettings,
  type InferenceMode,
  unsupportedInferenceMode,
} from "../src/providers/types.js";

class RequestCaptured extends Error {}

function settings(inferenceMode: InferenceMode): ConversationSettings {
  return {
    model: "model",
    systemPrompt: "system",
    tools: [],
    firstObservation: { text: "observation", cameras: {} },
    reasoningEffort: "high",
    inferenceMode,
  };
}

// WHY: Each fake client records the request body and stops before a reply is needed.
async function openaiRequest(mode: InferenceMode) {
  let body: Record<string, unknown> = {};
  const client = {
    responses: {
      create: (request: Record<string, unknown>) => {
        body = request;
        throw new RequestCaptured();
      },
    },
  } as unknown as OpenAI;
  await expect(
    new OpenAIConversation(settings(mode), client).nextTurn(),
  ).rejects.toThrow(RequestCaptured);
  return body;
}

function claudeRequest(mode: InferenceMode) {
  let body: Record<string, unknown> = {};
  const client = {
    beta: {
      messages: {
        thread: (request: Record<string, unknown>) => {
          body = request;
          return { append: () => {} };
        },
      },
    },
  } as unknown as Anthropic;
  new ClaudeConversation(settings(mode), client);
  return body;
}

async function geminiRequest(mode: InferenceMode) {
  let body: Record<string, unknown> = {};
  const client = {
    interactions: {
      create: (request: Record<string, unknown>) => {
        body = request;
        throw new RequestCaptured();
      },
    },
  } as unknown as GoogleGenAI;
  await expect(
    new GeminiConversation(settings(mode), client).nextTurn(),
  ).rejects.toThrow(RequestCaptured);
  return body;
}

test("inference mode availability depends on both the provider and the model", () => {
  expect(
    unsupportedInferenceMode("openai", "gpt-6-astra", "ultrafast"),
  ).toBeNull();
  expect(unsupportedInferenceMode("openai", "gpt-5.6", "ultrafast")).toContain(
    "gpt-5.6",
  );
  expect(
    unsupportedInferenceMode("anthropic", "claude-fable-5-2", "ultrafast"),
  ).toContain("not offered by anthropic");
  expect(
    unsupportedInferenceMode("anthropic", "claude-fable-5-2", "fast"),
  ).toBeNull();
  expect(
    unsupportedInferenceMode("anthropic", "claude-sonnet-5-5", "fast"),
  ).toContain("claude-sonnet-5-5");
  expect(
    unsupportedInferenceMode("gemini", "gemini-4-argon", "fast"),
  ).toBeNull();
});

test("OpenAI sends fast and ultrafast as the service tier, and standard leaves it unset", async () => {
  expect(await openaiRequest("standard")).not.toHaveProperty("service_tier");
  expect(await openaiRequest("fast")).toMatchObject({ service_tier: "fast" });
  expect(await openaiRequest("ultrafast")).toMatchObject({
    service_tier: "ultrafast",
  });
});

test("Claude sends fast as speed, and standard leaves it unset", () => {
  expect(claudeRequest("standard")).not.toHaveProperty("speed");
  expect(claudeRequest("fast")).toMatchObject({ speed: "fast" });
});

test("Gemini sends fast as the Priority tier, and standard leaves it unset", async () => {
  expect(await geminiRequest("standard")).not.toHaveProperty("service_tier");
  expect(await geminiRequest("fast")).toMatchObject({
    service_tier: "priority",
  });
});

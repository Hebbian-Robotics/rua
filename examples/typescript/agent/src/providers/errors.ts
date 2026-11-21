// What a provider raises once its retries are spent: the loop ends cleanly.

import Anthropic from "@anthropic-ai/sdk";
import { ApiError as GeminiApiError } from "@google/genai";
import OpenAI from "openai";

export class OpenAIResponseError extends Error {
  override name = "OpenAIResponseError";
}

export class GeminiInteractionError extends Error {
  override name = "GeminiInteractionError";
}

export function isProviderError(error: unknown): boolean {
  return (
    error instanceof Anthropic.APIError ||
    error instanceof OpenAI.APIError ||
    error instanceof OpenAIResponseError ||
    error instanceof GeminiApiError ||
    error instanceof GeminiInteractionError
  );
}

// What the loop and the three providers share: the conversation interface and
// the provider-neutral turn, usage and observation shapes.

import type {
  RobotObservation,
  RobotToolCall,
  RobotToolResult,
} from "hebbian-rua";

export const PROVIDERS = ["anthropic", "openai", "gemini"] as const;
export type Provider = (typeof PROVIDERS)[number];

export const REASONING_EFFORTS = ["low", "medium", "high", "xhigh"] as const;
export type ReasoningEffort = (typeof REASONING_EFFORTS)[number];
export const DEFAULT_REASONING_EFFORT: ReasoningEffort = "high";

export const DEFAULT_MODELS: Record<Provider, string> = {
  anthropic: "claude-fable-5-2",
  openai: "gpt-6.2-astra",
  gemini: "gemini-4-neon",
};

// CONSTRAINT: Gemini's thinking_level accepts xhigh only on these models
// (https://ai.google.dev/gemini-api/docs/thinking); the OpenAI and Anthropic
// default models accept it.
const GEMINI_XHIGH_MODELS = ["gemini-4-neon", "gemini-4-hydrogen"];

// WHY: Rejected before the run starts, rather than failing on the first model turn.
export function unsupportedReasoningEffort(
  provider: Provider,
  model: string | null,
  effort: ReasoningEffort,
): string | null {
  const resolved = model ?? DEFAULT_MODELS[provider];
  if (
    provider !== "gemini" ||
    effort !== "xhigh" ||
    GEMINI_XHIGH_MODELS.includes(resolved)
  )
    return null;
  return `Reasoning effort xhigh is not supported by ${resolved}; use high, or a Gemini model that supports it (${GEMINI_XHIGH_MODELS.join(", ")})`;
}

// Anthropic requires an explicit output limit; the other providers do not.
export const ANTHROPIC_MAX_OUTPUT_TOKENS = 128_000;
// Every SDK retries rate limits, overloads and dropped connections with backoff;
// set here rather than left to each SDK's default, so all three agree.
export const PROVIDER_MAX_RETRIES = 4;
// A high-effort turn with many frames can take minutes before the first byte.
export const PROVIDER_TIMEOUT_MILLISECONDS = 600_000;

export const API_KEY_VARIABLES: Record<Provider, string> = {
  anthropic: "ANTHROPIC_API_KEY",
  openai: "OPENAI_API_KEY",
  gemini: "GEMINI_API_KEY",
};

export interface ToolSpec {
  name: string;
  description: string;
  parametersSchema: Record<string, unknown>;
}

export type Observation = Pick<RobotObservation, "text" | "cameras">;

export interface TokenUsage {
  model: string;
  // CONSTRAINT: Prompt token counts include cached tokens.
  inputTokens: number;
  cachedInputTokens: number;
  // CONSTRAINT: Provider output billing includes thinking and reasoning tokens.
  outputTokens: number;

  reasoningTokens: number;
  cacheWriteTokens: number;
}

export interface ModelTurn {
  kind: "turn";
  toolCalls: RobotToolCall[];
  reasoningSummaries: string[];

  usage: TokenUsage | null;
}

export interface ModelRefusal {
  kind: "refusal";
  usage: TokenUsage | null;
}

export interface ModelConversation {
  nextTurn(signal?: AbortSignal): Promise<ModelTurn | ModelRefusal>;
  addToolResults(results: RobotToolResult[], observation?: Observation): void;
  addObservation(observation: Observation): void;
}

export interface ConversationSettings {
  model: string;
  systemPrompt: string;
  tools: ToolSpec[];
  firstObservation: Observation;
  reasoningEffort: ReasoningEffort;

  apiKey?: string;
}

export function encodedJpeg(jpeg: Uint8Array): string {
  return Buffer.from(jpeg).toString("base64");
}

export function parsedJsonArguments(argumentsText: string): unknown {
  try {
    return JSON.parse(argumentsText) as unknown;
  } catch {
    return argumentsText;
  }
}

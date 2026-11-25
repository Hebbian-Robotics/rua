import type { TracerProvider } from "@opentelemetry/api";

import type {
  RobotObservation,
  RobotToolCall,
  RobotToolResult,
} from "hebbian-rua";

export const PROVIDERS = ["openai", "gemini", "anthropic"] as const;
export type Provider = (typeof PROVIDERS)[number];

export const REASONING_EFFORTS = ["low", "medium", "high", "xhigh"] as const;
export type ReasoningEffort = (typeof REASONING_EFFORTS)[number];
export const DEFAULT_REASONING_EFFORT: ReasoningEffort = "high";

// CONSTRAINT: Gemini's thinking_level accepts xhigh only on these models
// (https://ai.google.dev/gemini-api/docs/thinking).
const GEMINI_XHIGH_MODELS = ["gemini-4-neon", "gemini-4-hydrogen"];

// WHY: Rejected before the run starts, rather than failing on the first model turn.
export function unsupportedReasoningEffort(
  provider: Provider,
  model: string,
  effort: ReasoningEffort,
): string | null {
  if (
    provider !== "gemini" ||
    effort !== "xhigh" ||
    GEMINI_XHIGH_MODELS.includes(model)
  )
    return null;
  return `Reasoning effort xhigh is not supported by ${model}; use high, or a Gemini model that supports it (${GEMINI_XHIGH_MODELS.join(", ")})`;
}

// CONSTRAINT: Faster, pricier serving of the same model. OpenAI has fast and ultrafast
// (service_tier); Anthropic has fast (speed); Gemini's nearest is its Priority
// tier (service_tier: "priority"), offered here as fast.
export const INFERENCE_MODES = ["standard", "fast", "ultrafast"] as const;
export type InferenceMode = (typeof INFERENCE_MODES)[number];
export const DEFAULT_INFERENCE_MODE: InferenceMode = "standard";

export const PROVIDER_INFERENCE_MODES: Record<
  Provider,
  readonly InferenceMode[]
> = {
  openai: ["standard", "fast", "ultrafast"],
  anthropic: ["standard", "fast"],
  gemini: ["standard", "fast"],
};

// CONSTRAINT: Only these models serve these modes
// (https://developers.openai.com/api/docs/guides/ultrafast-mode and Anthropic's
// fast mode docs). OpenAI fast and Gemini Priority cover their current models.
const INFERENCE_MODE_MODELS: Partial<
  Record<`${Provider}:${InferenceMode}`, string[]>
> = {
  "openai:ultrafast": [
    "gpt-6.2-astra",
    "gpt-6.2-sol",
    "gpt-6-astra",
    "gpt-6.1-sol",
  ],
  "anthropic:fast": [
    "claude-fable-5-2",
    "claude-mythos-5-2",
    "claude-opus-5-5",
  ],
};

// WHY: Rejected before the run starts, rather than failing on the first model turn.
export function unsupportedInferenceMode(
  provider: Provider,
  model: string,
  mode: InferenceMode,
): string | null {
  if (!PROVIDER_INFERENCE_MODES[provider].includes(mode))
    return `Inference mode ${mode} is not offered by ${provider}`;
  const models = INFERENCE_MODE_MODELS[`${provider}:${mode}`];
  if (models === undefined || models.includes(model)) return null;
  return `Inference mode ${mode} is not supported by ${model}; use standard, or a model that supports it (${models.join(", ")})`;
}

// CONSTRAINT: Anthropic requires an explicit output limit; the other providers do not.
export const ANTHROPIC_MAX_OUTPUT_TOKENS = 128_000;
// CONSTRAINT: Every SDK retries rate limits, overloads and dropped connections with backoff;
// set here rather than left to each SDK's default, so all three agree.
export const PROVIDER_MAX_RETRIES = 4;
// CONSTRAINT: A high-effort turn with many frames can take minutes before the first byte.
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

export type Observation = Pick<RobotObservation, "text" | "cameras"> & {
  referenceImageJpeg?: Uint8Array;
};

export function observationImages(observation: Observation) {
  const images = Object.entries(observation.cameras).map(([name, jpeg]) => ({
    label: `Camera: ${name}`,
    jpeg,
  }));
  if (observation.referenceImageJpeg)
    images.push({
      label: "User-provided reference image",
      jpeg: observation.referenceImageJpeg,
    });
  return images;
}

export type TokenPriceContext = { service_tier: string } | { speed: string };

export interface TokenUsage {
  model: string;
  // CONSTRAINT: Prompt token counts include cached tokens.
  inputTokens: number;
  cachedInputTokens: number;
  // CONSTRAINT: Provider output billing includes thinking and reasoning tokens.
  outputTokens: number;

  reasoningTokens: number;
  cacheWriteTokens: number;
  priceContext?: TokenPriceContext;
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
  tracerProvider?: TracerProvider;
  model: string;
  systemPrompt: string;
  tools: ToolSpec[];
  firstObservation: Observation;
  reasoningEffort: ReasoningEffort;
  inferenceMode?: InferenceMode;

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

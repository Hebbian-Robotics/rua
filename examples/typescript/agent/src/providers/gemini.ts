import { GoogleGenAI } from "@google/genai";
import type { RobotToolResult } from "hebbian-rua";
import { GeminiInteractionError } from "./errors.js";
import {
  type ConversationSettings,
  encodedJpeg,
  type ModelConversation,
  type ModelRefusal,
  type ModelTurn,
  type Observation,
  PROVIDER_MAX_RETRIES,
  PROVIDER_TIMEOUT_MILLISECONDS,
  type TokenUsage,
} from "./types.js";

// CONSTRAINT: The provider SDK does not export the interaction type by name; keep this local
// shape limited to the fields the loop consumes.
export interface GeminiInteraction {
  id: string;
  status?: string | undefined;
  model?: string | undefined;
  errors?: unknown;
  usage?:
    | {
        total_input_tokens?: number | undefined;
        total_cached_tokens?: number | undefined;
        total_output_tokens?: number | undefined;
        total_thought_tokens?: number | undefined;
      }
    | undefined;
  steps?: readonly GeminiStep[] | undefined;
}

type GeminiStep =
  | {
      type: "function_call";
      id: string;
      name: string;
      arguments: Record<string, unknown>;
    }
  | {
      type: "thought";
      summary?: ReadonlyArray<{ type: string; text?: string }> | undefined;
    }
  | { type: string };

export function geminiUsage(
  model: string,
  response: GeminiInteraction,
): TokenUsage | null {
  const { usage } = response;
  if (!usage) return null;
  const thought = usage.total_thought_tokens ?? 0;
  return {
    model: response.model ?? model,
    inputTokens: usage.total_input_tokens ?? 0,
    cachedInputTokens: usage.total_cached_tokens ?? 0,
    outputTokens: (usage.total_output_tokens ?? 0) + thought,
    reasoningTokens: thought,
    cacheWriteTokens: 0,
  };
}

export function geminiTurn(
  model: string,
  response: GeminiInteraction,
): ModelTurn | ModelRefusal {
  const usage = geminiUsage(model, response);
  if (
    (response.status !== "completed" &&
      response.status !== "requires_action") ||
    !response.id
  )
    throw new GeminiInteractionError(
      `Gemini interaction failed: ${response.status}: ${JSON.stringify(response.errors ?? response.usage)}`,
    );
  const toolCalls = [];
  const reasoningSummaries: string[] = [];
  for (const step of response.steps ?? []) {
    if (step.type === "function_call" && "id" in step && "arguments" in step)
      toolCalls.push({
        id: step.id,
        name: step.name,
        arguments: step.arguments,
      });
    else if (step.type === "thought" && "summary" in step)
      for (const part of step.summary ?? [])
        if (part.type === "text" && part.text)
          reasoningSummaries.push(part.text);
  }
  return { kind: "turn", toolCalls, reasoningSummaries, usage };
}

function observationInput(observation: Observation) {
  return {
    type: "user_input" as const,
    content: [
      ...Object.entries(observation.cameras).flatMap(([name, jpeg]) => [
        { type: "text", text: `Camera: ${name}` } as const,
        {
          type: "image",
          mime_type: "image/jpeg",
          data: encodedJpeg(jpeg),
        } as const,
      ]),
      { type: "text", text: observation.text } as const,
    ],
  };
}

type GeminiInput =
  | ReturnType<typeof observationInput>
  | ReturnType<typeof functionResultInput>;

function functionResultInput(result: RobotToolResult) {
  return {
    type: "function_result" as const,
    name: result.call.name,
    call_id: result.call.id,
    is_error: result.isError,
    result: [{ type: "text" as const, text: result.text }],
  };
}

export class GeminiConversation implements ModelConversation {
  private readonly client: GoogleGenAI;
  private readonly settings: ConversationSettings;
  private readonly tools;
  private previousInteractionId: string | null = null;
  private pendingInput: GeminiInput[];

  constructor(settings: ConversationSettings, client?: GoogleGenAI) {
    this.client =
      client ??
      new GoogleGenAI({
        apiKey: settings.apiKey,
        httpOptions: {
          timeout: PROVIDER_TIMEOUT_MILLISECONDS,
          retryOptions: { attempts: PROVIDER_MAX_RETRIES },
        },
      });
    this.settings = settings;
    this.tools = settings.tools.map((tool) => ({
      type: "function" as const,
      name: tool.name,
      description: tool.description,
      parameters: tool.parametersSchema,
    }));
    this.pendingInput = [observationInput(settings.firstObservation)];
  }

  async nextTurn(signal?: AbortSignal): Promise<ModelTurn | ModelRefusal> {
    const response: GeminiInteraction = await this.client.interactions.create(
      {
        model: this.settings.model,
        input: this.pendingInput,
        ...(this.previousInteractionId === null
          ? {}
          : { previous_interaction_id: this.previousInteractionId }),
        // Chaining by previous_interaction_id needs the stored interaction.
        store: true,
        // tools, system_instruction and generation_config are interaction-scoped:
        // resent identically every turn so the implicit cache sees a stable prefix.
        system_instruction: this.settings.systemPrompt,
        tools: this.tools,
        generation_config: {
          thinking_level: this.settings.reasoningEffort,
          thinking_summaries: "auto",
        },
      },
      signal === undefined
        ? undefined
        : {
            signal: AbortSignal.any([
              signal,
              AbortSignal.timeout(PROVIDER_TIMEOUT_MILLISECONDS),
            ]),
          },
    );
    const turn = geminiTurn(this.settings.model, response);
    this.previousInteractionId = response.id;
    this.pendingInput = [];
    return turn;
  }

  addToolResults(results: RobotToolResult[], observation?: Observation): void {
    this.pendingInput.push(...results.map(functionResultInput));
    for (const result of results) {
      if (result.cameras && Object.keys(result.cameras).length > 0)
        this.addObservation({
          text: `Images requested by tool call ${result.call.id}.\n${result.text}`,
          cameras: result.cameras,
        });
    }
    if (observation) this.addObservation(observation);
  }

  addObservation(observation: Observation): void {
    this.pendingInput.push(observationInput(observation));
  }
}

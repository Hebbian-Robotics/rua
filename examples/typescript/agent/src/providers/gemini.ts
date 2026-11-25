import { GoogleGenAI, type Interactions } from "@google/genai";
import type { RobotToolResult } from "hebbian-rua";
import { PendingModelImages } from "../model-inputs.js";
import { GeminiInteractionError } from "./errors.js";
import {
  type ConversationSettings,
  encodedJpeg,
  type ModelConversation,
  type ModelRefusal,
  type ModelTurn,
  type Observation,
  observationImages,
  PROVIDER_MAX_RETRIES,
  PROVIDER_TIMEOUT_MILLISECONDS,
  type TokenUsage,
} from "./types.js";

export type GeminiInteraction = Pick<
  Interactions.Interaction,
  "id" | "status" | "model" | "errors" | "usage" | "steps" | "service_tier"
>;

export function geminiUsage(
  model: string,
  response: GeminiInteraction,
): TokenUsage | null {
  const { usage, service_tier: serviceTier } = response;
  if (!usage) return null;
  const thought = usage.total_thought_tokens ?? 0;
  return {
    model: response.model ?? model,
    inputTokens: usage.total_input_tokens ?? 0,
    cachedInputTokens: usage.total_cached_tokens ?? 0,
    outputTokens: (usage.total_output_tokens ?? 0) + thought,
    reasoningTokens: thought,
    cacheWriteTokens: 0,
    ...(serviceTier ? { priceContext: { service_tier: serviceTier } } : {}),
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
    if (step.type === "function_call")
      toolCalls.push({
        id: step.id,
        name: step.name,
        arguments: step.arguments,
      });
    else if (step.type === "thought")
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
      ...observationImages(observation).flatMap(({ label, jpeg }) => [
        { type: "text", text: label } as const,
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
  private readonly modelInputs = new PendingModelImages();
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
    this.modelInputs.add(settings.firstObservation.cameras);
    this.pendingInput = [observationInput(settings.firstObservation)];
  }

  async nextTurn(signal?: AbortSignal): Promise<ModelTurn | ModelRefusal> {
    await this.modelInputs.save();
    const response: GeminiInteraction = await this.client.interactions.create(
      {
        model: this.settings.model,
        input: this.pendingInput,
        ...(this.previousInteractionId === null
          ? {}
          : { previous_interaction_id: this.previousInteractionId }),
        // CONSTRAINT: Chaining by previous_interaction_id needs the stored interaction.
        store: true,
        // CONSTRAINT: tools, system_instruction and generation_config are interaction-scoped:
        // resent identically every turn so the implicit cache sees a stable prefix.
        system_instruction: this.settings.systemPrompt,
        tools: this.tools,
        generation_config: {
          thinking_level: this.settings.reasoningEffort,
          thinking_summaries: "auto",
        },
        // CONSTRAINT: Gemini's Priority tier is its fast mode; over its limits a request runs at standard.
        ...(this.settings.inferenceMode === "fast"
          ? { service_tier: "priority" }
          : {}),
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
        this.addObservation(
          {
            text: `Images requested by tool call ${result.call.id}.\n${result.text}`,
            cameras: result.cameras,
          },
          result.call.id,
        );
    }
    if (observation) this.addObservation(observation);
  }

  addObservation(observation: Observation, toolCallId?: string): void {
    this.modelInputs.add(observation.cameras, toolCallId);
    this.pendingInput.push(observationInput(observation));
  }
}

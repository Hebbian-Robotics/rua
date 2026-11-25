import Anthropic from "@anthropic-ai/sdk";
import type {
  BetaContentBlockParam,
  BetaMessage,
} from "@anthropic-ai/sdk/resources/beta/messages";
import type { RobotToolResult } from "hebbian-rua";
import { PendingModelImages } from "../model-inputs.js";
import {
  ANTHROPIC_MAX_OUTPUT_TOKENS,
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

export function claudeUsage(response: BetaMessage): TokenUsage {
  const { usage } = response;
  const cacheRead = usage.cache_read_input_tokens ?? 0;
  const cacheWrite = usage.cache_creation_input_tokens ?? 0;
  return {
    model: response.model,
    // CONSTRAINT: Claude's input_tokens excludes cache reads and writes.
    inputTokens: usage.input_tokens + cacheRead + cacheWrite,
    cachedInputTokens: cacheRead,
    outputTokens: usage.output_tokens,
    // CONSTRAINT: Thinking is billed as output; the usage does not split it out.
    reasoningTokens: 0,
    cacheWriteTokens: cacheWrite,
    ...(usage.speed ? { priceContext: { speed: usage.speed } } : {}),
  };
}

export function claudeTurn(response: BetaMessage): ModelTurn | ModelRefusal {
  const usage = claudeUsage(response);
  if (response.stop_reason === "refusal") return { kind: "refusal", usage };
  return {
    kind: "turn",
    toolCalls: response.content.flatMap((block) =>
      block.type === "tool_use"
        ? [{ id: block.id, name: block.name, arguments: block.input }]
        : [],
    ),
    reasoningSummaries: response.content.flatMap((block) =>
      block.type === "thinking" && block.thinking ? [block.thinking] : [],
    ),
    usage,
  };
}

function observationBlocks(observation: Observation): BetaContentBlockParam[] {
  return [
    ...observationImages(observation).flatMap(
      ({ label, jpeg }): BetaContentBlockParam[] => [
        { type: "text", text: label },
        {
          type: "image",
          source: {
            type: "base64",
            media_type: "image/jpeg",
            data: encodedJpeg(jpeg),
          },
        },
      ],
    ),
    { type: "text", text: observation.text },
  ];
}

export class ClaudeConversation implements ModelConversation {
  private readonly modelInputs = new PendingModelImages();
  private readonly thread;

  constructor(settings: ConversationSettings, client?: Anthropic) {
    const anthropic =
      client ??
      new Anthropic({
        apiKey: settings.apiKey,
        maxRetries: PROVIDER_MAX_RETRIES,
        timeout: PROVIDER_TIMEOUT_MILLISECONDS,
        openTelemetry: settings.tracerProvider
          ? {
              tracerProvider: settings.tracerProvider,
              propagation: false,
              traces: { contentMode: "metadata_only" },
            }
          : false,
      });
    this.thread = anthropic.beta.messages.thread({
      model: settings.model,
      max_tokens: ANTHROPIC_MAX_OUTPUT_TOKENS,
      system: settings.systemPrompt,
      tools: settings.tools.map((tool) => ({
        name: tool.name,
        description: tool.description,
        input_schema: { type: "object", ...tool.parametersSchema },
      })),
      output_config: { effort: settings.reasoningEffort },
      thinking: { type: "adaptive", display: "summarized" },
      ...(settings.inferenceMode === "fast" ? { speed: "fast" as const } : {}),
      // CONSTRAINT: Let the API advance the cache breakpoint as the thread grows.
      cache_control: { type: "ephemeral" },
      // WHY: If the API cannot thread a request, send each turn's full history instead.
      unsupportedRequestBehavior: "continue_unthreaded",
      betas: ["server-side-fallback-2026-07-01"],
      fallbacks: "default",
    });
    this.addObservation(settings.firstObservation);
  }

  async nextTurn(signal?: AbortSignal): Promise<ModelTurn | ModelRefusal> {
    await this.modelInputs.save();
    // CONSTRAINT: Streamed: a high-effort turn with a large max_tokens can outlast a buffered
    // request's timeout. Aborting the signal ends the stream.
    const stream = this.thread.stream(undefined, undefined, { signal });
    return claudeTurn(await stream.finalMessage());
  }

  addToolResults(results: RobotToolResult[], observation?: Observation): void {
    for (const result of results)
      if (result.cameras) this.modelInputs.add(result.cameras, result.call.id);
    if (observation) this.modelInputs.add(observation.cameras);
    this.thread.append({
      role: "user",
      content: [
        ...results.map(
          (result): BetaContentBlockParam => ({
            type: "tool_result",
            tool_use_id: result.call.id,
            content: result.text,
            is_error: result.isError,
          }),
        ),
        ...results.flatMap((result) =>
          result.cameras && Object.keys(result.cameras).length > 0
            ? observationBlocks({
                text: `Images requested by tool call ${result.call.id}.\n${result.text}`,
                cameras: result.cameras,
              })
            : [],
        ),
        ...(observation ? observationBlocks(observation) : []),
      ],
    });
  }

  addObservation(observation: Observation): void {
    this.modelInputs.add(observation.cameras);
    this.thread.append({
      role: "user",
      content: observationBlocks(observation),
    });
  }
}

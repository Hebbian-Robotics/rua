import { randomUUID } from "node:crypto";
import type { RobotToolResult } from "hebbian-rua";
import OpenAI from "openai";
import type {
  FunctionTool,
  Response,
  ResponseInputContent,
  ResponseInputItem,
} from "openai/resources/responses/responses";
import { PendingModelImages } from "../model-inputs.js";
import { OpenAIResponseError } from "./errors.js";
import {
  type ConversationSettings,
  encodedJpeg,
  type ModelConversation,
  type ModelRefusal,
  type ModelTurn,
  type Observation,
  PROVIDER_MAX_RETRIES,
  PROVIDER_TIMEOUT_MILLISECONDS,
  parsedJsonArguments,
  type TokenUsage,
} from "./types.js";

export function openaiUsage(response: Response): TokenUsage | null {
  const { usage } = response;
  if (!usage) return null;
  return {
    model: response.model,
    inputTokens: usage.input_tokens,
    cachedInputTokens: usage.input_tokens_details.cached_tokens,
    outputTokens: usage.output_tokens,
    reasoningTokens: usage.output_tokens_details.reasoning_tokens,
    cacheWriteTokens: usage.input_tokens_details.cache_write_tokens ?? 0,
  };
}

export function openaiTurn(response: Response): ModelTurn | ModelRefusal {
  if (
    response.status === "failed" ||
    response.status === "incomplete" ||
    response.status === "cancelled"
  ) {
    const detail = response.incomplete_details ?? response.error;
    throw new OpenAIResponseError(
      `OpenAI response ${response.status}: ${JSON.stringify(detail)}`,
    );
  }
  const usage = openaiUsage(response);
  const toolCalls = response.output.flatMap((item) =>
    item.type === "function_call"
      ? [
          {
            id: item.call_id,
            name: item.name,
            arguments: parsedJsonArguments(item.arguments),
          },
        ]
      : [],
  );
  const refused = response.output.some(
    (item) =>
      item.type === "message" &&
      item.content.some((part) => part.type === "refusal"),
  );
  if (refused && toolCalls.length === 0) return { kind: "refusal", usage };
  return {
    kind: "turn",
    toolCalls,
    reasoningSummaries: response.output.flatMap((item) =>
      item.type === "reasoning" ? item.summary.map((part) => part.text) : [],
    ),
    usage,
  };
}

function observationMessage(observation: Observation): ResponseInputItem {
  const content: ResponseInputContent[] = [
    ...Object.entries(observation.cameras).flatMap(
      ([name, jpeg]): ResponseInputContent[] => [
        { type: "input_text", text: `Camera: ${name}` },
        {
          type: "input_image",
          image_url: `data:image/jpeg;base64,${encodedJpeg(jpeg)}`,
          detail: "auto",
        },
      ],
    ),
    { type: "input_text", text: observation.text },
  ];
  return { role: "user", content };
}

// CONSTRAINT: Responses history is chained by response ID; dropping old observation images
// would require resending the history.
export class OpenAIConversation implements ModelConversation {
  private readonly modelInputs = new PendingModelImages();
  private readonly client: OpenAI;
  private readonly settings: ConversationSettings;
  private readonly tools: FunctionTool[];
  private previousResponseId: string | null = null;
  // Routes every turn of this run to the same cache; the prefix is already stable.
  private readonly promptCacheKey = `hebbian-agent-${randomUUID()}`;
  // Items not yet sent: the next request carries only these.
  private pendingInput: ResponseInputItem[];

  constructor(settings: ConversationSettings, client?: OpenAI) {
    this.client =
      client ??
      new OpenAI({
        apiKey: settings.apiKey,
        maxRetries: PROVIDER_MAX_RETRIES,
        timeout: PROVIDER_TIMEOUT_MILLISECONDS,
      });
    this.settings = settings;
    this.tools = settings.tools.map((tool) => ({
      type: "function",
      name: tool.name,
      description: tool.description,
      parameters: tool.parametersSchema,
      // Strict mode needs every property required; the loop validates.
      strict: false,
    }));
    this.modelInputs.add(settings.firstObservation.cameras);
    this.pendingInput = [observationMessage(settings.firstObservation)];
  }

  async nextTurn(signal?: AbortSignal): Promise<ModelTurn | ModelRefusal> {
    await this.modelInputs.save();
    const response = await this.client.responses.create(
      {
        model: this.settings.model,
        // Instructions do not carry over a previous_response_id chain.
        instructions: this.settings.systemPrompt,
        input: this.pendingInput,
        tools: this.tools,
        ...(this.previousResponseId === null
          ? {}
          : { previous_response_id: this.previousResponseId }),
        // The server keeps the history and its reasoning items for previous_response_id.
        store: true,
        prompt_cache_key: this.promptCacheKey,
        reasoning: { effort: this.settings.reasoningEffort, summary: "auto" },
      },
      { signal },
    );
    const turn = openaiTurn(response);
    this.previousResponseId = response.id;
    this.pendingInput = [];
    return turn;
  }

  addToolResults(results: RobotToolResult[], observation?: Observation): void {
    // The output has no error flag; the text says rejected or invalid.
    for (const result of results)
      this.pendingInput.push({
        type: "function_call_output",
        call_id: result.call.id,
        output: result.text,
      });
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
    this.pendingInput.push(observationMessage(observation));
  }
}

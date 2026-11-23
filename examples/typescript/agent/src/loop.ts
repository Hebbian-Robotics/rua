import type { Span } from "@opentelemetry/api";
import {
  ArmRecoveryEndedError,
  apiSchemas,
  createRecordingClient,
  createRobotTools,
  filterSequenceActions,
  LeaseLostError,
  type OnInterruption,
  type RobotObservation,
  type StationConnection,
  StationConnectionError,
  StationHttpError,
  StationTimeoutError,
} from "hebbian-rua";
import { type RunEnvironment, reportEvent } from "./environment.js";
import { createAgentToolResult, runToolBatch } from "./execution.js";
import { observe, type RevisedObservation } from "./observation.js";
import { isProviderError } from "./providers/errors.js";
import { startConversation } from "./providers/index.js";
import {
  type ConversationSettings,
  DEFAULT_MODELS,
  type ModelConversation,
  type ModelRefusal,
  type ModelTurn,
  type Observation,
  type Provider,
  type ReasoningEffort,
  type TokenUsage,
} from "./providers/types.js";
import { AGENT_TOOLS, systemPrompt, TaskMemory } from "./tools.js";
import { KIND_ATTRIBUTE, markSpanError, withStationTrace } from "./tracing.js";
import { estimateModelCost, RunUsage, type UsageSnapshot } from "./usage.js";

const MAXIMUM_LEASE_LABEL_CHARACTERS = 200;

export interface LoopConfig {
  arm?: string;
  task: string;

  runId?: string;

  cameras: string[];

  optionalCameras: string[];
  provider: Provider;

  model: string | null;
  maxModelTurns?: number;
  reasoningEffort: ReasoningEffort;

  onInterruption: OnInterruption;

  record: boolean;
}

export interface StationEndpoints {
  connection: StationConnection;
  stationUrl: string;
  mediaUrl: string;
}

export interface DriveOptions {
  openConversation?: (
    firstObservation: Observation,
    settings: ConversationSettings,
  ) => ModelConversation;

  signal?: AbortSignal;
  apiKey?: string;
  print?: (line: string) => void;
}

export interface DriveResult {
  result: string;

  cancelled: boolean;

  incomplete: boolean;
  usage: UsageSnapshot;
}

interface LoopEnding {
  result: string;
  incomplete: boolean;
}

const finishedEnding = (result: string): LoopEnding => ({
  result,
  incomplete: false,
});
const incompleteEnding = (result: string): LoopEnding => ({
  result,
  incomplete: true,
});

class RunCancelledError extends Error {
  constructor() {
    super("The run was cancelled");
  }
}

async function awaitModelTurn(
  conversation: ModelConversation,
  signal: AbortSignal | undefined,
): Promise<ModelTurn | ModelRefusal> {
  if (signal === undefined) return conversation.nextTurn();
  if (signal.aborted) throw new RunCancelledError();
  let onAbort: (() => void) | undefined;
  const cancelled = new Promise<never>((_resolve, reject) => {
    onAbort = () => reject(new RunCancelledError());
    signal.addEventListener("abort", onAbort, { once: true });
  });
  try {
    return await Promise.race([conversation.nextTurn(signal), cancelled]);
  } finally {
    if (onAbort !== undefined) signal.removeEventListener("abort", onAbort);
  }
}

const CANCELLED_RESULT = "Run cancelled; the arm was stopped";

type ResolvedLoopConfig = LoopConfig & { arm: string; model: string };

export function requirePositiveModelTurnLimit(maxModelTurns: number): number {
  if (!Number.isSafeInteger(maxModelTurns) || maxModelTurns < 1) {
    throw new RangeError("The model turn limit must be a positive integer");
  }
  return maxModelTurns;
}

async function resolveSoleArm(connection: StationConnection): Promise<string> {
  const { arms } = await connection.state();
  const [soleArm, ...otherArms] = arms;
  if (soleArm === undefined) throw new Error("The station reports no arms");
  if (otherArms.length > 0) {
    throw new Error(
      `Select an arm from: ${arms.map(({ arm }) => arm).join(", ")}`,
    );
  }
  return soleArm.arm;
}

export async function drive(
  requested: LoopConfig,
  endpoints: StationEndpoints,
  options: DriveOptions = {},
): Promise<DriveResult> {
  if (requested.maxModelTurns !== undefined)
    requirePositiveModelTurnLimit(requested.maxModelTurns);
  const { connection } = endpoints;
  const config: ResolvedLoopConfig = {
    ...requested,
    arm: requested.arm ?? (await resolveSoleArm(connection)),
    model: requested.model ?? DEFAULT_MODELS[requested.provider],
  };
  const usage = new RunUsage(config.runId);
  const print = options.print ?? ((line: string) => console.log(line));
  const label = config.task.slice(0, MAXIMUM_LEASE_LABEL_CHARACTERS);
  const leaseOptions = {
    requiredCameras: config.cameras,
    onInterruption: config.onInterruption,
  };
  const lease = await connection.acquireLease(label, leaseOptions);
  let leaseLost = false;
  const stopListeningForLease = connection.onLeaseLost(() => {
    leaseLost = true;
  });
  const robot = createRobotTools({
    connection,
    lease,
    mediaUrl: endpoints.mediaUrl,
    arm: config.arm,
    cameras: config.cameras,
    optionalCameras: config.optionalCameras,
    ...(options.signal === undefined ? {} : { signal: options.signal }),
  }).mapTools(filterSequenceActions((action) => action !== "joints"));
  let cancellationStop: Promise<void> | undefined;
  const stopOnAbort = (): void => {
    // Return the lease even if another operation is still waiting to finish.
    cancellationStop = lease
      .stopArm(config.arm)
      .catch(() => undefined)
      .finally(() => lease.return());
  };
  options.signal?.addEventListener("abort", stopOnAbort, { once: true });
  if (options.signal?.aborted) stopOnAbort();
  try {
    const recording = createRecordingClient(endpoints.stationUrl);
    if (config.record) await recording.ensure();
    let taskEnded = false;
    let cancelled = false;
    let incomplete = false;
    const result = await withStationTrace(
      connection,
      "invoke_agent hebbian",
      {
        [KIND_ATTRIBUTE]: "run",
        "gen_ai.operation.name": "invoke_agent",
        "gen_ai.provider.name": config.provider,
        "gen_ai.request.model": config.model,
        "hebbian.arm": config.arm,
        "hebbian.task": config.task,
      },
      async (runSpan, withSpan) => {
        const environment: RunEnvironment = {
          connection,
          robot,
          lease,
          withSpan,
          memory: new TaskMemory(config.task),
          arm: config.arm,
          cameras: config.cameras,
          leaseLost: () => leaseLost,
          print,
        };
        const outcome = await runLoop(config, environment, usage, options).then(
          (ending) => {
            taskEnded = true;
            incomplete = ending.incomplete;
            return ending.result;
          },
          (error: unknown) => {
            if (error instanceof ArmRecoveryEndedError) {
              incomplete = true;
              return "The arm faulted while waiting for recovery";
            }
            if (error instanceof LeaseLostError) {
              incomplete = true;
              return "The lease was lost: taken, revoked, returned for an interruption or lapsed";
            }
            if (error instanceof RunCancelledError) {
              cancelled = true;
              return CANCELLED_RESULT;
            }
            throw error;
          },
        );
        runSpan.setAttribute("hebbian.result", outcome);
        return outcome;
      },
    );
    if (taskEnded) await lease.stopRecording();
    return { result, cancelled, incomplete, usage: usage.snapshot() };
  } finally {
    options.signal?.removeEventListener("abort", stopOnAbort);
    await cancellationStop;
    robot.close();
    stopListeningForLease();
    await lease.return();
  }
}

function throwIfInterrupted(
  environment: RunEnvironment,
  signal: AbortSignal | undefined,
): void {
  if (signal?.aborted) throw new RunCancelledError();
  if (environment.leaseLost()) throw new LeaseLostError();
}

async function runLoop(
  config: ResolvedLoopConfig,
  environment: RunEnvironment,
  usage: RunUsage,
  options: DriveOptions,
): Promise<LoopEnding> {
  const { connection, robot, lease } = environment;
  reportEvent(connection, {
    type: "run_started",
    task: config.task,
    arm: config.arm,
    provider: config.provider,
    model: config.model,
    cameras: config.cameras.length > 0 ? config.cameras.join(",") : null,
    optional_cameras:
      config.optionalCameras.length > 0
        ? config.optionalCameras.join(",")
        : null,
    speed_limit: null,
  });
  const first = await observe(
    environment,
    `Task: ${config.task}\nThe station's robot profile supplies the default and maximum joint speeds.`,
  );
  const settings: ConversationSettings = {
    model: config.model,
    systemPrompt: systemPrompt(JSON.stringify(await robot.context())),
    tools: [
      ...robot.tools.map((tool) => ({
        name: tool.name,
        description: tool.description,
        parametersSchema: tool.parameters as Record<string, unknown>,
      })),
      ...AGENT_TOOLS,
    ],
    firstObservation: first.observation,
    reasoningEffort: config.reasoningEffort,
    ...(options.apiKey === undefined ? {} : { apiKey: options.apiKey }),
  };
  const conversation =
    options.openConversation?.(first.observation, settings) ??
    startConversation(config.provider, settings);
  let ending: LoopEnding;
  try {
    ending = await runTurns(
      config,
      environment,
      conversation,
      usage,
      first,
      options.signal,
    );
  } catch (error) {
    if (options.signal?.aborted) {
      throw new RunCancelledError();
    }
    if (!isProviderError(error)) throw error;
    if (environment.leaseLost()) throw new LeaseLostError();
    // Stop the arm rather than leave a motion queued after a provider error.
    await lease.stopArm(config.arm);
    ending = incompleteEnding(
      `Model provider error; loop stopped: ${error instanceof Error ? error.message : String(error)}`,
    );
  }
  const summary = usage.snapshot();
  environment.print(
    `${summary.turns} model calls, ${summary.input_tokens} input (${summary.cached_input_tokens} cached) and ${summary.output_tokens} output tokens`,
  );
  reportEvent(connection, {
    type: "run_finished",
    result: ending.result,
    usage: summary,
  });
  return ending;
}

function armRevisionChangedSinceObservation(
  state: Awaited<ReturnType<StationConnection["state"]>>,
  arm: string,
  observation: RevisedObservation,
): boolean {
  const current = state.arms.find((snapshot) => snapshot.arm === arm);
  return (
    current !== undefined && current.revision !== observation.revisions[arm]
  );
}

export function wireTurn(turn: ModelTurn | ModelRefusal) {
  const usage = turn.usage && wireUsage(turn.usage);
  return turn.kind === "refusal"
    ? { usage }
    : {
        tool_calls: turn.toolCalls.map((call) => ({
          call_id: call.id,
          name: call.name,
          arguments: call.arguments,
        })),
        reasoning_summaries: turn.reasoningSummaries,
        usage,
      };
}

function wireUsage(usage: TokenUsage) {
  return {
    model: usage.model,
    input_tokens: usage.inputTokens,
    cached_input_tokens: usage.cachedInputTokens,
    output_tokens: usage.outputTokens,
    reasoning_tokens: usage.reasoningTokens,
    cache_write_tokens: usage.cacheWriteTokens,
  };
}

async function setTurnAttributes(
  span: Span,
  turn: ModelTurn | ModelRefusal,
  provider: Provider,
): Promise<void> {
  if (turn.kind === "turn") {
    span.setAttribute(
      "hebbian.tool_calls",
      turn.toolCalls.map((call) => call.name),
    );
    if (turn.reasoningSummaries.length > 0)
      span.setAttribute("hebbian.reasoning_summaries", turn.reasoningSummaries);
  } else {
    span.setAttribute("hebbian.tool_calls", []);
    markSpanError(span, "The model declined the task");
  }
  if (turn.usage) {
    const estimatedCost = await estimateModelCost(provider, turn.usage);
    if (estimatedCost !== null)
      span.setAttribute("hebbian.estimated_cost_usd", estimatedCost);
    span.setAttributes({
      "gen_ai.response.model": turn.usage.model,
      "gen_ai.usage.input_tokens": turn.usage.inputTokens,
      "gen_ai.usage.output_tokens": turn.usage.outputTokens,
      "gen_ai.usage.cache_read.input_tokens": turn.usage.cachedInputTokens,
      "hebbian.usage.reasoning_tokens": turn.usage.reasoningTokens,
    });
  }
}

async function runTurns(
  config: ResolvedLoopConfig,
  environment: RunEnvironment,
  conversation: ModelConversation,
  usage: RunUsage,
  firstObservation: RevisedObservation,
  signal: AbortSignal | undefined,
): Promise<LoopEnding> {
  const { connection, robot, lease, withSpan, arm } = environment;
  let lastObservation = firstObservation;
  let modelTurns = 0;
  while (
    config.maxModelTurns === undefined ||
    modelTurns < config.maxModelTurns
  ) {
    throwIfInterrupted(environment, signal);
    if (
      armRevisionChangedSinceObservation(
        await connection.state(),
        arm,
        lastObservation,
      )
    ) {
      lastObservation = await observe(
        environment,
        "Observation after an interruption.",
      );
      conversation.addObservation(lastObservation.observation);
      continue;
    }
    const faultedArms = (await connection.state()).arms.filter(
      (snapshot) => snapshot.mode === "faulted",
    );
    if (faultedArms.length > 0)
      return incompleteEnding(
        `Arm fault; loop stopped: ${faultedArms.map((snapshot) => `${snapshot.arm}: ${snapshot.fault}`).join("; ")}`,
      );
    modelTurns += 1;
    const turn = await withSpan(
      `chat ${config.provider}`,
      {
        [KIND_ATTRIBUTE]: "model",
        "gen_ai.operation.name": "chat",
        "gen_ai.provider.name": config.provider,
        "gen_ai.request.model": config.model,
      },
      async (span) => {
        const modelTurn = await awaitModelTurn(conversation, signal);
        await setTurnAttributes(span, modelTurn, config.provider);
        return modelTurn;
      },
    );
    throwIfInterrupted(environment, signal);
    reportEvent(connection, { type: "model_turn", turn: wireTurn(turn) });
    if (turn.usage) {
      usage.turns.push(turn.usage);
      reportEvent(connection, {
        type: "usage_update",
        usage: usage.snapshot(),
      });
    }
    if (turn.kind === "refusal")
      return incompleteEnding(`The ${config.provider} model declined the task`);
    if (turn.toolCalls.length === 0) {
      lastObservation = await observe(
        environment,
        "Call a tool: action_sequence, preview_pose, update_plan, observe, stop or finish.",
      );
      conversation.addObservation(lastObservation.observation);
      continue;
    }
    const stateAfterTurn = await connection.state();
    const armState = stateAfterTurn.arms.find(
      (snapshot) => snapshot.arm === arm,
    );
    const armDisconnected =
      armState?.mode === "disconnected" || armState?.mode === "reconnected";
    if (
      armDisconnected ||
      armRevisionChangedSinceObservation(stateAfterTurn, arm, lastObservation)
    ) {
      if (armDisconnected && !(await robot.waitForArmRecovery()))
        return incompleteEnding("The arm faulted while waiting for recovery");
      const staleResults = turn.toolCalls.map((call) =>
        createAgentToolResult(
          call,
          "The arm was interrupted before this command ran. Re-evaluate from the fresh observation.",
          "error",
        ),
      );
      lastObservation = await observe(
        environment,
        "Observation after an interruption.",
      );
      conversation.addToolResults(staleResults, lastObservation.observation);
      continue;
    }
    let outcome: Awaited<ReturnType<typeof runToolBatch>>;
    try {
      outcome = await runToolBatch(
        environment,
        turn.toolCalls,
        lastObservation.revisions,
      );
    } catch (error) {
      if (
        !(
          error instanceof StationConnectionError ||
          error instanceof StationHttpError ||
          error instanceof StationTimeoutError
        )
      )
        throw error;
      for (const snapshot of (await connection.state()).arms)
        await lease.stopArm(snapshot.arm);
      return incompleteEnding(
        `Station communication error; loop stopped: ${error.message}`,
      );
    }
    for (const result of outcome.results)
      if (result.halt)
        return incompleteEnding(`Arm fault; loop stopped: ${result.text}`);
    if (outcome.finishReason !== null)
      return finishedEnding(outcome.finishReason);
    if (
      outcome.results.some((result) => result.interrupted) &&
      !(await robot.waitForArmRecovery())
    )
      return incompleteEnding("The arm faulted while waiting for recovery");
    if (outcome.needsObservation) {
      let alreadyCaptured: RobotObservation | undefined;
      const onlyReadTools = outcome.results.every((result) =>
        ["observe", "preview_pose", "update_plan", "update_notes"].includes(
          result.call.name,
        ),
      );
      if (onlyReadTools) {
        const currentResults = outcome.results.filter(
          (result) =>
            result.call.name === "observe" &&
            !result.isError &&
            !result.sequenceImages,
        );
        const current =
          currentResults.length === 1 ? currentResults[0] : undefined;
        const parsed = apiSchemas.shape.StationState.safeParse(current?.data);
        if (current && parsed.success) {
          alreadyCaptured = {
            data: parsed.data,
            text: current.text,
            cameras: current.cameras ?? {},
            revisions: Object.fromEntries(
              parsed.data.arms.map((arm) => [arm.arm, arm.revision]),
            ),
          };
        }
      }
      lastObservation = await observe(
        environment,
        "Observation after the tools ran.",
        alreadyCaptured,
      );
      const results = alreadyCaptured
        ? outcome.results.map((result) =>
            result.call.name === "observe" &&
            !result.isError &&
            !result.sequenceImages
              ? { ...result, text: "ok", data: "ok", cameras: {} }
              : result,
          )
        : outcome.results;
      conversation.addToolResults(results, lastObservation.observation);
    } else {
      conversation.addToolResults(outcome.results);
    }
  }
  return incompleteEnding(`No task outcome after ${modelTurns} model turns`);
}

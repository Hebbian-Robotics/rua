import type { Span } from "@opentelemetry/api";
import {
  parseModelData,
  type RobotToolCall,
  type RobotToolResult,
  type ToolCallWrapper,
} from "hebbian-rua";
import { z } from "zod";
import { type RunEnvironment, reportEvent } from "./environment.js";
import {
  FINISH_TOOL,
  finishInputSchema,
  isMemoryTool,
  type MemoryToolName,
  UPDATE_NOTES_TOOL,
  type UpdatePlanInput,
  updateNotesInputSchema,
  updatePlanInputSchema,
} from "./tools.js";
import {
  KIND_ATTRIBUTE,
  markSpanError,
  traceContextForSpan,
} from "./tracing.js";

type MemoryToolCall = RobotToolCall & { name: MemoryToolName };

interface IndexedCall<Call extends RobotToolCall = RobotToolCall> {
  index: number;
  call: Call;
}

function isMemoryToolCall(call: RobotToolCall): call is MemoryToolCall {
  return isMemoryTool(call.name);
}

type BatchPlan =
  | { kind: "reject_finish" }
  | { kind: "finish"; call: RobotToolCall }
  | {
      kind: "run";
      memoryCalls: IndexedCall<MemoryToolCall>[];
      robotCalls: IndexedCall[];
    };

// INVARIANT: Apply plan/notes updates in call order before motor calls so telemetry boundaries
// are recorded before motion; finish must be the only call in its batch.
function planToolBatch(calls: readonly RobotToolCall[]): BatchPlan {
  const finish = calls.find((call) => call.name === FINISH_TOOL);
  if (finish)
    return calls.length === 1
      ? { kind: "finish", call: finish }
      : { kind: "reject_finish" };
  const plan: BatchPlan = {
    kind: "run",
    memoryCalls: [],
    robotCalls: [],
  };
  for (const [index, call] of calls.entries()) {
    if (isMemoryToolCall(call)) plan.memoryCalls.push({ index, call });
    else plan.robotCalls.push({ index, call });
  }
  return plan;
}

export function createAgentToolResult(
  call: RobotToolCall,
  text: string,
  outcome: "success" | "error",
): RobotToolResult {
  return {
    call,
    text,
    data: text,
    get modelData() {
      return parseModelData(this.text);
    },
    isError: outcome === "error",
    halt: false,
    interrupted: false,
  };
}

function invalidInput(call: RobotToolCall, error: z.ZodError): RobotToolResult {
  return createAgentToolResult(
    call,
    `Invalid input: ${z.prettifyError(error)}`,
    "error",
  );
}

function runMemoryTool(
  environment: RunEnvironment,
  call: MemoryToolCall,
  span: Span,
): RobotToolResult {
  const update = parseMemoryUpdate(call.name, call.arguments);
  const timestamp = (BigInt(Date.now()) * 1_000_000n).toString();
  const coordinates = { timestamp_unix_ns: timestamp, tool_call_id: call.id };
  const { connection, memory } = environment;
  switch (update.kind) {
    case "invalid":
      return invalidInput(call, update.error);
    case "notes_update":
      memory.notes = update.notes;
      span.setAttributes({
        "hebbian.notes": update.notes,
        "hebbian.notes.updated_at_unix_ns": timestamp,
        "hebbian.memory": memory.viewerSnapshot(),
      });
      reportEvent(connection, {
        type: update.kind,
        notes: update.notes,
        ...coordinates,
      });
      break;
    case "plan_update":
      memory.plan = update.plan;
      span.setAttributes({
        "hebbian.plan.steps": JSON.stringify(update.plan.steps),
        "hebbian.plan.note": update.plan.note ?? "",
        "hebbian.plan.updated_at_unix_ns": timestamp,
        "hebbian.memory": memory.viewerSnapshot(),
      });
      reportEvent(connection, {
        type: update.kind,
        steps: update.plan.steps,
        note: update.plan.note ?? null,
        ...coordinates,
      });
      break;
  }
  return createAgentToolResult(call, "ok", "success");
}

type MemoryUpdate =
  | { kind: "invalid"; error: z.ZodError }
  | { kind: "notes_update"; notes: string[] }
  | { kind: "plan_update"; plan: UpdatePlanInput };

function parseMemoryUpdate(
  name: MemoryToolName,
  argumentsValue: unknown,
): MemoryUpdate {
  if (name === UPDATE_NOTES_TOOL) {
    const parsed = updateNotesInputSchema.safeParse(argumentsValue);
    return parsed.success
      ? { kind: "notes_update", notes: parsed.data.notes }
      : { kind: "invalid", error: parsed.error };
  }
  const parsed = updatePlanInputSchema.safeParse(argumentsValue);
  return parsed.success
    ? { kind: "plan_update", plan: parsed.data }
    : { kind: "invalid", error: parsed.error };
}

function tracedCall(
  environment: RunEnvironment,
  call: RobotToolCall,
  run: (span: Span) => Promise<RobotToolResult>,
): Promise<RobotToolResult> {
  return environment.withSpan(
    `execute_tool ${call.name}`,
    {
      [KIND_ATTRIBUTE]: "tool",
      "gen_ai.operation.name": "execute_tool",
      "gen_ai.tool.name": call.name,
      "gen_ai.tool.call.id": call.id,
      "gen_ai.tool.call.arguments": JSON.stringify(call.arguments),
    },
    async (span) => {
      const result = await run(span);
      span.setAttribute("gen_ai.tool.call.result", result.text);
      if (result.isError) markSpanError(span, result.text);
      environment.print(
        `${call.name} ${JSON.stringify(call.arguments)} -> ${result.text}`,
      );
      return result;
    },
  );
}

type BatchOutcome = { results: RobotToolResult[] } & (
  | { kind: "finished"; reason: string }
  | { kind: "observe" }
  | { kind: "continue" }
);

export async function runToolBatch(
  environment: RunEnvironment,
  calls: readonly RobotToolCall[],
  revisions: Record<string, number>,
): Promise<BatchOutcome> {
  const plan = planToolBatch(calls);
  let results: RobotToolResult[];
  let finish: string | null = null;
  if (plan.kind === "reject_finish") {
    results = calls.map((call) =>
      createAgentToolResult(
        call,
        "No calls executed: finish must be the only call in its turn.",
        "error",
      ),
    );
  } else if (plan.kind === "finish") {
    const { call } = plan;
    const parsed = finishInputSchema.safeParse(call.arguments);
    results = [
      await tracedCall(environment, call, async () =>
        parsed.success
          ? createAgentToolResult(call, "ok", "success")
          : invalidInput(call, parsed.error),
      ),
    ];
    if (parsed.success) finish = parsed.data.reason;
  } else {
    const resultsByCallIndex = new Map<number, RobotToolResult>();
    for (const { index, call } of plan.memoryCalls) {
      resultsByCallIndex.set(
        index,
        await tracedCall(environment, call, async (span) =>
          runMemoryTool(environment, call, span),
        ),
      );
    }
    const wrap: ToolCallWrapper = (call, run) =>
      tracedCall(environment, call, (span) => run(traceContextForSpan(span)));
    const robotCalls = plan.robotCalls.map(({ call }) => call);
    if (new Set(robotCalls.map((call) => call.id)).size !== robotCalls.length)
      throw new Error("Robot tool call IDs must be unique within a batch");
    const robotResults = await environment.robot.execute(
      robotCalls,
      revisions,
      wrap,
    );
    const robotResultsById = new Map(
      robotResults.map((result) => [result.call.id, result]),
    );
    for (const { index, call } of plan.robotCalls) {
      const result = robotResultsById.get(call.id);
      if (!result) throw new Error(`Missing result for tool call ${call.id}`);
      resultsByCallIndex.set(index, result);
    }
    results = calls.map((call, index) => {
      const result = resultsByCallIndex.get(index);
      if (!result) throw new Error(`Missing result for tool call ${call.id}`);
      return result;
    });
  }
  for (const result of results)
    reportEvent(environment.connection, {
      type: "tool_result",
      result: wireResult(result),
    });
  if (finish !== null) return { kind: "finished", reason: finish, results };
  if (
    plan.kind === "run" &&
    plan.robotCalls.some(({ call }) => needsObservationAfter(call))
  )
    return { kind: "observe", results };
  return { kind: "continue", results };
}

function needsObservationAfter(call: RobotToolCall): boolean {
  if (call.name === "preview_pose") return false;
  const argumentsValue = call.arguments;
  const historyOnly =
    call.name === "observe" &&
    typeof argumentsValue === "object" &&
    argumentsValue !== null &&
    "source" in argumentsValue &&
    argumentsValue.source === "last_sequence";
  return !historyOnly;
}

function wireResult(result: RobotToolResult) {
  return {
    call: {
      call_id: result.call.id,
      name: result.call.name,
      arguments: result.call.arguments,
    },
    text: result.text,
    is_error: result.isError,
    halt: result.halt,
    interrupted: result.interrupted,
  };
}

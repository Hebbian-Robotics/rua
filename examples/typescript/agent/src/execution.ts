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
  MEMORY_TOOLS,
  type TaskMemory,
  UPDATE_NOTES_TOOL,
  updateNotesInputSchema,
  updatePlanInputSchema,
} from "./tools.js";
import { KIND_ATTRIBUTE, type Span } from "./tracing.js";

export type BatchPlan =
  | { kind: "reject_finish" }
  | {
      kind: "run";
      memoryIndexes: number[];
      finishIndexes: number[];
      robotIndexes: number[];
    };

// INVARIANT: Apply plan/notes updates in call order before motor calls so telemetry boundaries
// are recorded before motion; finish must be the only call in its batch.
export function planToolBatch(calls: readonly RobotToolCall[]): BatchPlan {
  if (calls.length > 1 && calls.some((call) => call.name === FINISH_TOOL))
    return { kind: "reject_finish" };
  const indexesWhere = (matches: (name: string) => boolean): number[] =>
    calls.flatMap((call, index) => (matches(call.name) ? [index] : []));
  const memoryIndexes = indexesWhere((name) => MEMORY_TOOLS.includes(name));
  const finishIndexes = indexesWhere((name) => name === FINISH_TOOL);
  const robotIndexes = indexesWhere(
    (name) => !MEMORY_TOOLS.includes(name) && name !== FINISH_TOOL,
  );
  return { kind: "run", memoryIndexes, finishIndexes, robotIndexes };
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
  call: RobotToolCall,
  span: Span,
  unixNanoseconds: string,
): RobotToolResult {
  const { memory } = environment;
  const payload = applyMemoryUpdate(call, memory, unixNanoseconds);
  if (payload.kind === "invalid") return invalidInput(call, payload.error);
  span.setAttributes({
    ...payload.spanAttributes,
    "hebbian.memory": memory.viewerSnapshot(),
  });
  reportEvent(environment.connection, {
    type: payload.eventType,
    ...payload.body,
    timestamp_unix_ns: unixNanoseconds,
    tool_call_id: call.id,
  });
  return createAgentToolResult(call, "ok", "success");
}

type MemoryUpdate =
  | { kind: "invalid"; error: z.ZodError }
  | {
      kind: "applied";
      eventType: "plan_update" | "notes_update";
      body: Record<string, unknown>;
      spanAttributes: Record<string, string | string[]>;
    };

function applyMemoryUpdate(
  call: RobotToolCall,
  memory: TaskMemory,
  unixNanoseconds: string,
): MemoryUpdate {
  if (call.name === UPDATE_NOTES_TOOL) {
    const parsed = updateNotesInputSchema.safeParse(call.arguments);
    if (!parsed.success) return { kind: "invalid", error: parsed.error };
    memory.notes = parsed.data.notes;
    return {
      kind: "applied",
      eventType: "notes_update",
      body: { notes: parsed.data.notes },
      spanAttributes: {
        "hebbian.notes": parsed.data.notes,
        "hebbian.notes.updated_at_unix_ns": unixNanoseconds,
      },
    };
  }
  const parsed = updatePlanInputSchema.safeParse(call.arguments);
  if (!parsed.success) return { kind: "invalid", error: parsed.error };
  memory.plan = parsed.data;
  const body = { steps: parsed.data.steps, note: parsed.data.note ?? null };
  return {
    kind: "applied",
    eventType: "plan_update",
    body,
    spanAttributes: {
      "hebbian.plan.steps": JSON.stringify(body.steps),
      "hebbian.plan.note": body.note ?? "",
      "hebbian.plan.updated_at_unix_ns": unixNanoseconds,
    },
  };
}

function tracedCall(
  environment: RunEnvironment,
  call: RobotToolCall,
  run: (span: Span) => Promise<RobotToolResult>,
): Promise<RobotToolResult> {
  return environment.tracer.span(
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
      if (result.isError) span.setError(result.text);
      environment.print(
        `${call.name} ${JSON.stringify(call.arguments)} -> ${result.text}`,
      );
      return result;
    },
  );
}

export interface BatchOutcome {
  results: RobotToolResult[];
  finishReason: string | null;
  needsObservation: boolean;
}

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
  } else {
    const resultsByCallIndex = new Map<number, RobotToolResult>();
    for (const index of plan.memoryIndexes) {
      const call = calls[index] as RobotToolCall;
      resultsByCallIndex.set(
        index,
        await tracedCall(environment, call, async (span) =>
          runMemoryTool(
            environment,
            call,
            span,
            environment.tracer.clock().toString(),
          ),
        ),
      );
    }
    for (const index of plan.finishIndexes) {
      const call = calls[index] as RobotToolCall;
      resultsByCallIndex.set(
        index,
        await tracedCall(environment, call, async () => {
          const parsed = finishInputSchema.safeParse(call.arguments);
          if (!parsed.success) return invalidInput(call, parsed.error);
          finish = parsed.data.reason;
          return createAgentToolResult(call, "ok", "success");
        }),
      );
    }
    const wrap: ToolCallWrapper = (call, run) =>
      tracedCall(environment, call, (span) => run(span.traceContext()));
    const robotCalls = plan.robotIndexes.map(
      (index) => calls[index] as RobotToolCall,
    );
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
    for (const index of plan.robotIndexes) {
      const call = calls[index] as RobotToolCall;
      const result = robotResultsById.get(call.id);
      if (!result) throw new Error(`Missing result for tool call ${call.id}`);
      resultsByCallIndex.set(index, result);
    }
    results = calls.map(
      (_call, index) => resultsByCallIndex.get(index) as RobotToolResult,
    );
  }
  for (const result of results)
    reportEvent(environment.connection, {
      type: "tool_result",
      result: wireResult(result),
    });
  const needsObservation =
    plan.kind === "run" &&
    plan.robotIndexes.some((index) => {
      const call = calls[index];
      if (!call || call.name === "preview_pose") return false;
      const argumentsValue = call.arguments;
      const historyOnly =
        call.name === "observe" &&
        typeof argumentsValue === "object" &&
        argumentsValue !== null &&
        "source" in argumentsValue &&
        argumentsValue.source === "last_sequence";
      return !historyOnly;
    });
  return { results, finishReason: finish, needsObservation };
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

import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { createStationConnection } from "hebbian-rua";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { drive, type LoopConfig } from "../src/loop.js";
import { OpenAIResponseError } from "../src/providers/errors.js";
import {
  type Received,
  ScriptedConversation,
  ScriptedStation,
  turn,
} from "./station-harness.js";

const config: LoopConfig = {
  arm: "follower",
  task: "Touch the block",
  cameras: ["cam"],
  optionalCameras: [],
  provider: "anthropic",
  model: "claude-fable-5-2",
  reasoningEffort: "high",
  onInterruption: "wait_for_approval",
  record: false,
};

let station: ScriptedStation;

beforeEach(() => {
  station = new ScriptedStation();
  station.install();
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.unstubAllEnvs();
});

async function runLoop(
  steps: ConstructorParameters<typeof ScriptedConversation>[0],
  overrides: Partial<LoopConfig> = {},
  signal?: AbortSignal,
) {
  const connection = createStationConnection("http://station", "agent");
  connection.start();
  const conversation = new ScriptedConversation(steps);
  const output: { line: string; requestsSent: number }[] = [];
  try {
    const outcome = await station.run(
      drive(
        { ...config, ...overrides },
        { connection, stationUrl: "http://station" },
        {
          openConversation: () => conversation,
          print: (line) =>
            output.push({ line, requestsSent: station.requests.length }),
          ...(signal ? { signal } : {}),
        },
      ),
    );
    return { ...outcome, conversation, output };
  } finally {
    connection.close();
  }
}

test("Claude request telemetry is parented to the model turn without model content", async () => {
  vi.stubEnv("ANTHROPIC_BASE_URL", "http://claude.test");
  const modelInputs = await mkdtemp(join(tmpdir(), "rua-model-inputs-test-"));
  vi.stubEnv("HEBBIAN_RUA_MODEL_INPUTS_DIR", modelInputs);
  let submittedRequest: string | undefined;
  const stationFetch = globalThis.fetch;
  const modelEvents = [
    {
      type: "message_start",
      message: {
        id: "msg_1",
        type: "message",
        role: "assistant",
        content: [],
        model: "claude-sonnet-4-5",
        stop_reason: null,
        stop_sequence: null,
        usage: { input_tokens: 1, output_tokens: 0 },
      },
    },
    {
      type: "content_block_start",
      index: 0,
      content_block: {
        type: "tool_use",
        id: "call_1",
        name: "finish",
        input: {},
      },
    },
    {
      type: "content_block_delta",
      index: 0,
      delta: {
        type: "input_json_delta",
        partial_json: JSON.stringify({ reason: "private result" }),
      },
    },
    { type: "content_block_stop", index: 0 },
    {
      type: "message_delta",
      delta: { stop_reason: "tool_use", stop_sequence: null },
      usage: { output_tokens: 1 },
    },
    { type: "message_stop" },
  ];
  vi.stubGlobal(
    "fetch",
    (input: string | URL | Request, init?: RequestInit) => {
      const url =
        input instanceof Request ? new URL(input.url) : new URL(input);
      if (url.host !== "claude.test") return stationFetch(input, init);
      if (typeof init?.body === "string") submittedRequest = init.body;
      return Promise.resolve(
        new Response(
          modelEvents
            .map(
              (event) =>
                `event: ${event.type}\ndata: ${JSON.stringify(event)}\n\n`,
            )
            .join(""),
          { headers: { "Content-Type": "text/event-stream" } },
        ),
      );
    },
  );

  const connection = createStationConnection("http://station", "agent");
  connection.start();
  try {
    const outcome = await station.run(
      drive(
        { ...config, task: "private prompt" },
        { connection, stationUrl: "http://station" },
        { apiKey: "local-test-key" },
      ),
    );
    expect(outcome.result).toBe("private result");

    const spans = station.finishedSpans();
    const modelSpan = spans.find((span) => span.name === "chat anthropic");
    const requestSpans = spans.filter(
      (span) => span.parent_span_id === modelSpan?.span_id,
    );
    const manifest = modelSpan?.attributes["hebbian.model_inputs.path"];
    expect(typeof manifest).toBe("string");
    if (typeof manifest !== "string")
      throw new Error("Missing model input index");
    expect(manifest).toBe(
      join(modelInputs, modelSpan?.trace_id ?? "", "turn-0001", "index.json"),
    );
    expect(JSON.parse(await readFile(manifest, "utf8"))).toEqual({
      turn: 1,
      images: [{ file: "1.jpg", camera: "cam" }],
    });
    const jpeg = await readFile(join(dirname(manifest), "1.jpg"));
    expect(jpeg).toEqual(Buffer.from([1, 2, 3]));
    expect(submittedRequest).toContain(jpeg.toString("base64"));
    expect(requestSpans).toHaveLength(1);
    expect(requestSpans[0]).toMatchObject({ trace_id: modelSpan?.trace_id });
    expect(JSON.stringify(requestSpans)).not.toMatch(
      /private prompt|private result/,
    );
  } finally {
    connection.close();
    await rm(modelInputs, { recursive: true, force: true });
  }
});

function resultsOf(received: Received | undefined) {
  if (received?.kind !== "results") throw new Error("Expected tool results");
  return received;
}

test("a plan, a motion and a finish run end to end and are reported to the station", async () => {
  const plan = { steps: [{ text: "touch", status: "active" }] };
  const notes = { notes: ["Red block"] };
  const preview = { poses: [{ pose: { position_mm: [200, 0, 100] } }] };
  const motionTurn = turn(
    ["stop", {}],
    ["update_notes", notes],
    ["preview_pose", preview],
    ["update_plan", plan],
  );
  motionTurn.reasoningSummaries = ["Touch the red block"];
  const { result, conversation, output } = await runLoop([
    motionTurn,
    turn(["finish", { reason: "Touched the block" }]),
  ]);
  expect(result).toBe("Touched the block");
  expect(station.returned).toBe(true);

  const types = station.events.map((event) => event["type"]);
  expect(types[0]).toBe("run_started");
  expect(types.at(-1)).toBe("run_finished");
  expect(types).toEqual(
    expect.arrayContaining([
      "observation",
      "model_turn",
      "usage_update",
      "tool_result",
      "plan_update",
    ]),
  );
  expect(station.eventsOfType("model_turn")[0]).toEqual({
    type: "model_turn",
    turn: {
      tool_calls: [
        { call_id: "call-stop-0", name: "stop", arguments: {} },
        {
          call_id: "call-update_notes-1",
          name: "update_notes",
          arguments: notes,
        },
        {
          call_id: "call-preview_pose-2",
          name: "preview_pose",
          arguments: preview,
        },
        { call_id: "call-update_plan-3", name: "update_plan", arguments: plan },
      ],
      reasoning_summaries: ["Touch the red block"],
      usage: {
        model: "scripted",
        input_tokens: 10,
        cached_input_tokens: 4,
        output_tokens: 5,
        reasoning_tokens: 2,
        cache_write_tokens: 0,
      },
    },
  });
  expect(station.eventsOfType("run_finished")[0]).toMatchObject({
    result: "Touched the block",
    usage: {
      turns: 2,
      input_tokens: 20,
      cached_input_tokens: 8,
      output_tokens: 10,
      reasoning_tokens: 4,
    },
  });
  expect(output.at(-1)?.line).toBe(
    "2 model calls, 20 input (8 cached) and 10 output tokens",
  );
  const commandRequest = station.requests.indexOf("POST /commands");
  const previewRequest = station.requests.indexOf("POST /pose_previews");
  expect(commandRequest).toBeGreaterThanOrEqual(0);
  expect(previewRequest).toBeGreaterThanOrEqual(0);
  const firstRobotRequest = Math.min(commandRequest, previewRequest);
  expect(output[0]?.line).toMatch(/^update_notes /);
  expect(output[1]?.line).toMatch(/^update_plan /);
  for (const entry of output.slice(0, 2)) {
    expect(entry.requestsSent).toBeLessThanOrEqual(firstRobotRequest);
  }

  const first = resultsOf(conversation.received[0]);
  expect(first.calls.map((call) => call.id)).toEqual([
    "call-stop-0",
    "call-update_notes-1",
    "call-preview_pose-2",
    "call-update_plan-3",
  ]);
  expect(first.errors).toEqual([false, false, false, false]);
  expect(first.texts[2]).toContain("No reachable pose");
  expect(first.observation?.text).toContain("Current plan:\n1. [active] touch");
  expect(first.observation?.text).toContain("- Red block");
  expect(Object.keys(first.observation?.cameras ?? {})).toEqual(["cam"]);

  const finished = station.finishedSpans();
  const stopSpan = finished.find((span) => span.name === "execute_tool stop");
  const traceparent = station.commands[0]?.trace_context?.traceparent;
  expect(traceparent).toBe(`00-${stopSpan?.trace_id}-${stopSpan?.span_id}-01`);

  expect(finished.map((span) => span.name)).toEqual(
    expect.arrayContaining([
      "observe",
      "chat anthropic",
      "execute_tool stop",
      "execute_tool update_notes",
      "execute_tool preview_pose",
      "execute_tool update_plan",
      "execute_tool finish",
      "invoke_agent hebbian",
    ]),
  );
  const root = finished.find((span) => span.name === "invoke_agent hebbian");
  expect(root?.parent_span_id).toBeNull();
  expect(root?.attributes["hebbian.result"]).toBe("Touched the block");
  expect(
    finished
      .filter((span) => span !== root)
      .every((span) => span.parent_span_id === root?.span_id),
  ).toBe(true);
  for (const span of finished) {
    const updates = station.spans.filter(
      (update) => update.span_id === span.span_id,
    );
    expect(
      updates.map((update) => update.attributes["hebbian.active"]),
    ).toEqual([true, undefined]);
    expect(updates[0]?.start_time_unix_nano).toBe(span.start_time_unix_nano);
    expect(BigInt(span.end_time_unix_nano)).toBeGreaterThanOrEqual(
      BigInt(span.start_time_unix_nano),
    );
  }
});

test("planning and previews return results without fresh images; observe refreshes once", async () => {
  const { conversation } = await runLoop([
    turn(
      ["update_plan", { steps: [{ text: "inspect", status: "active" }] }],
      ["update_notes", { notes: ["block is left"] }],
    ),
    turn([
      "preview_pose",
      { poses: [{ pose: { position_mm: [200, 0, 100] } }] },
    ]),
    turn(["observe", { wait_seconds: 0 }]),
    turn(["finish", { reason: "done" }]),
  ]);
  const memory = resultsOf(conversation.received[0]);
  expect(memory.observation).toBeUndefined();
  expect(memory.texts).toEqual(["ok", "ok"]);
  const preview = resultsOf(conversation.received[1]);
  expect(preview.errors).toEqual([false]);
  expect(preview.texts[0]).toContain("No reachable pose");
  expect(preview.observation).toBeUndefined();
  const refresh = resultsOf(conversation.received[2]);
  expect(refresh.texts).toEqual(["ok"]);
  expect(refresh.observation?.text).toContain(
    "Current plan:\n1. [active] inspect",
  );
  expect(refresh.observation?.text).toContain("block is left");
  expect(Object.keys(refresh.observation?.cameras ?? {})).toEqual(["cam"]);
  expect(
    station.requests.filter((request) => request.includes("/observations")),
  ).toHaveLength(2);
});

test("finish must be alone, so the rest of its turn is refused", async () => {
  const { result, conversation } = await runLoop([
    turn(
      ["update_plan", { steps: [{ text: "inspect", status: "active" }] }],
      ["finish", { reason: "done" }],
      ["stop", {}],
    ),
    turn(["finish", { reason: "first" }], ["finish", { reason: "second" }]),
    turn(["finish", { reason: "really done" }]),
  ]);
  expect(result).toBe("really done");
  expect(station.commands).toHaveLength(0);
  expect(station.eventsOfType("plan_update")).toHaveLength(0);
  const mixed = resultsOf(conversation.received[0]);
  const repeated = resultsOf(conversation.received[1]);
  expect(mixed.errors).toEqual([true, true, true]);
  expect(repeated.errors).toEqual([true, true]);
  expect([...mixed.texts, ...repeated.texts]).toEqual(
    Array(5).fill(expect.stringContaining("finish must be the only call")),
  );
});

test("a turn without tool calls gets a nudge observation", async () => {
  const { conversation } = await runLoop([
    turn(),
    turn(["finish", { reason: "ok" }]),
  ]);
  const nudge = conversation.received[0];
  expect(nudge?.kind).toBe("observation");
  expect(nudge?.observation?.text).toContain(
    "Call a tool: action_sequence, preview_pose, update_plan, observe, stop or finish.",
  );
});

test("a run can finish after more than 20 model turns by default", async () => {
  const { result, incomplete } = await runLoop([
    ...Array.from({ length: 20 }, () => turn()),
    turn(["finish", { reason: "finished after 20 turns" }]),
  ]);
  expect(result).toBe("finished after 20 turns");
  expect(incomplete).toBe(false);
});

test("a model that keeps replying without an outcome ends at an explicit turn limit", async () => {
  const { result, incomplete } = await runLoop(
    [turn(), turn(), turn(["finish", { reason: "too late" }])],
    { maxModelTurns: 2 },
  );
  expect(incomplete).toBe(true);
  expect(result).toBe("No task outcome after 2 model turns");
});

test("a refusal ends the run with the provider named", async () => {
  const { result } = await runLoop([{ kind: "refusal", usage: null }]);
  expect(result).toBe("The anthropic model declined the task");
  expect(station.eventsOfType("model_turn")).toEqual([
    { type: "model_turn", turn: { usage: null } },
  ]);
  const span = station
    .finishedSpans()
    .find((candidate) => candidate.name === "chat anthropic");
  expect(span?.status.message).toBe("The model declined the task");
});

test("a provider error stops the arm and ends the run", async () => {
  const { result } = await runLoop([
    new OpenAIResponseError("OpenAI response failed"),
  ]);
  expect(result).toBe(
    "Model provider error; loop stopped: OpenAI response failed",
  );
  expect(station.leaseStops).toContain("follower");
  expect(
    station.finishedSpans().find((span) => span.name === "chat anthropic")
      ?.status,
  ).toEqual({
    code: "STATUS_CODE_ERROR",
    message: "OpenAI response failed",
  });
});

test("an observation that went stale gets error results and a fresh observation, not a command", async () => {
  const { conversation, result } = await runLoop([
    () => {
      station.revision = 1;
      station.emitState();
      return turn(["stop", {}]);
    },
    turn(["finish", { reason: "recovered" }]),
  ]);
  expect(result).toBe("recovered");
  expect(station.commands).toHaveLength(0);
  const stale = resultsOf(conversation.received[0]);
  expect(stale.texts[0]).toContain("The arm was interrupted");
  expect(stale.observation?.text).toContain(
    "Observation after an interruption.",
  );
});

test("an arm that faulted ends the run during recovery", async () => {
  station.armMode = "faulted";
  const { result } = await runLoop([]);
  expect(result).toBe("The arm faulted while waiting for recovery");
});

test("losing the lease ends the run without stopping the recording", async () => {
  const { result } = await runLoop(
    [
      () => {
        station.loseLease();
        return turn(["stop", {}]);
      },
    ],
    { record: true },
  );
  expect(result).toBe(
    "The lease was lost: taken, revoked, returned for an interruption or lapsed",
  );
  expect(station.requests).toContain("POST /recording/ensure");
  expect(station.requests).not.toContain("POST /recording/stop-for-lease");
  expect(station.commands).toHaveLength(0);
});

test("a normal finish stops the recording the run started", async () => {
  await runLoop([turn(["finish", { reason: "ok" }])], { record: true });
  expect(station.requests).toContain("POST /recording/stop-for-lease");
});

test("cancelling stops the arm and leaves the recording running", async () => {
  const cancellation = new AbortController();
  const { result } = await runLoop(
    [
      () => {
        cancellation.abort();
        return turn(["stop", {}]);
      },
    ],
    { record: true },
    cancellation.signal,
  );
  expect(result).toBe("Run cancelled; the arm was stopped");
  expect(station.commands).toHaveLength(0);
  expect(station.leaseStops).toContain("follower");
  expect(station.requests).not.toContain("POST /recording/stop-for-lease");
  expect(station.returned).toBe(true);
});

test("cancellation returns the lease while a model request is still pending", async () => {
  const connection = createStationConnection("http://station", "agent");
  connection.start();
  const abort = new AbortController();
  let completeModelTurn: () => void = () => {};
  const conversation = new ScriptedConversation([]);
  conversation.nextTurn = () =>
    new Promise((resolve) => {
      completeModelTurn = () => resolve(turn(["stop", {}]));
      abort.abort();
    });
  const running = drive(
    config,
    { connection, stationUrl: "http://station" },
    { signal: abort.signal, openConversation: () => conversation },
  );
  try {
    await station.run(
      new Promise<void>((resolve) => {
        const checkLease = () => {
          if (station.returned) resolve();
          else setImmediate(checkLease);
        };
        checkLease();
      }),
    );
    expect(station.leaseStops).toContain("follower");
    const result = await station.run(running);
    expect(result.cancelled).toBe(true);
    // INVARIANT: A late provider response must not execute any of its proposed tools.
    completeModelTurn();
    await new Promise<void>((resolve) => setImmediate(resolve));
    expect(station.eventsOfType("tool_result")).toEqual([]);
    expect(station.commands).toHaveLength(0);
  } finally {
    connection.close();
  }
});

test.each([
  ["standard", 0.00028],
  ["fast", 0.00056],
] as const)(
  "records the requested %s estimate when Claude omits speed",
  async (inferenceMode, estimate) => {
    const response = turn(["finish", { reason: "done" }]);
    if (response.usage === null) throw new Error("Expected token usage");
    response.usage.model = "claude-fable-5-2";
    response.usage.cachedInputTokens = 0;
    await runLoop([response], { inferenceMode });
    const modelSpan = station
      .finishedSpans()
      .find((span) => span.name === "chat anthropic");
    expect(modelSpan?.attributes["hebbian.estimated_cost_usd"]).toBeCloseTo(
      estimate,
      10,
    );
  },
);

test("a history-only observe returns its result without another live capture", async () => {
  const { conversation } = await runLoop([
    turn(["observe", { source: "last_sequence" }]),
    turn(["finish", { reason: "no retained sequence" }]),
  ]);
  const review = resultsOf(conversation.received[0]);
  expect(review.errors).toEqual([true]);
  expect(review.texts[0]).toContain("No sequence image history is available");
  expect(review.observation).toBeUndefined();
  expect(
    station.requests.filter((request) => request.includes("/observations")),
  ).toHaveLength(1);
});

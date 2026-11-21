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
  model: null,
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
});

async function runLoop(
  steps: ConstructorParameters<typeof ScriptedConversation>[0],
  overrides: Partial<LoopConfig> = {},
  signal?: AbortSignal,
) {
  const connection = createStationConnection("http://station", "agent");
  connection.start();
  const conversation = new ScriptedConversation(steps);
  const lines: string[] = [];
  try {
    const outcome = await station.run(
      drive(
        { ...config, ...overrides },
        { connection, stationUrl: "http://station", mediaUrl: "http://media" },
        {
          openConversation: () => conversation,
          print: (line) => lines.push(line),
          ...(signal ? { signal } : {}),
        },
      ),
    );
    return { ...outcome, conversation, lines };
  } finally {
    connection.close();
  }
}

function resultsOf(received: Received | undefined) {
  if (received?.kind !== "results") throw new Error("Expected tool results");
  return received;
}

test("a plan, a motion and a finish run end to end and are reported to the station", async () => {
  const { result, conversation, lines } = await runLoop([
    turn(
      ["stop", {}],
      ["update_plan", { steps: [{ text: "touch", status: "active" }] }],
    ),
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
  expect(lines.at(-1)).toBe(
    "2 model calls, 20 input (8 cached) and 10 output tokens",
  );

  // Results came back in call order, with the plan at the top of the observation.
  const first = resultsOf(conversation.received[0]);
  expect(first.calls.map((call) => call.name)).toEqual(["stop", "update_plan"]);
  expect(first.observation?.text).toContain("Current plan:\n1. [active] touch");
  expect(Object.keys(first.observation?.cameras ?? {})).toEqual(["cam"]);

  // The station command is parented to the stop call's execute_tool span.
  const finished = station.finishedSpans();
  const stopSpan = finished.find((span) => span.name === "execute_tool stop");
  const traceparent = station.commands[0]?.trace_context?.traceparent;
  expect(traceparent).toBe(`00-${stopSpan?.trace_id}-${stopSpan?.span_id}-01`);

  expect(finished.map((span) => span.name)).toEqual(
    expect.arrayContaining([
      "observe",
      "chat anthropic",
      "execute_tool stop",
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
  // Initial capture, then explicit observe; no captures for planning or preview.
  expect(station.eventsOfType("observation")).toHaveLength(2);
  expect(
    station.requests.filter((request) => request.includes("/cameras/")),
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
  expect(station.requests).toContain("POST /viewer/api/recording/start");
  expect(station.requests).not.toContain(
    "POST /viewer/api/recording/stop-for-lease",
  );
  expect(station.commands).toHaveLength(0);
});

test("a normal finish stops the recording the run started", async () => {
  await runLoop([turn(["finish", { reason: "ok" }])], { record: true });
  expect(station.requests).toContain(
    "POST /viewer/api/recording/stop-for-lease",
  );
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
  expect(station.requests).not.toContain(
    "POST /viewer/api/recording/stop-for-lease",
  );
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
      completeModelTurn = () => resolve(turn(["finish", { reason: "done" }]));
      abort.abort();
    });
  const running = drive(
    config,
    { connection, stationUrl: "http://station", mediaUrl: "http://media" },
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
    // A late provider response must not execute any of its proposed tools.
    completeModelTurn();
    await Promise.resolve();
    expect(station.commands).toHaveLength(0);
  } finally {
    connection.close();
  }
});

test("a priced model response records an estimate on its trace", async () => {
  const response = turn(["finish", { reason: "done" }]);
  if (response.usage === null) throw new Error("Expected token usage");
  response.usage.model = "claude-fable-5-1";
  await runLoop([response]);
  const modelSpan = station
    .finishedSpans()
    .find((span) => span.name === "chat anthropic");
  expect(modelSpan?.attributes["hebbian.estimated_cost_usd"]).toBeCloseTo(
    0.000271,
  );
});

test("a history-only observe returns its result without another live capture", async () => {
  const { conversation } = await runLoop([
    turn(["observe", { source: "last_sequence" }]),
    turn(["finish", { reason: "no retained sequence" }]),
  ]);
  const review = resultsOf(conversation.received[0]);
  expect(review.errors).toEqual([true]);
  expect(review.texts[0]).toContain("No sequence image history is available");
  expect(review.observation).toBeUndefined();
  expect(station.eventsOfType("observation")).toHaveLength(1);
  expect(
    station.requests.filter((request) => request.includes("/cameras/")),
  ).toHaveLength(1);
});

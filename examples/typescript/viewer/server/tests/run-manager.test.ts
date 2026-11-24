import { afterEach, beforeEach, expect, test, vi } from "vitest";
import type { ModelConversation } from "../../../agent/src/providers/types.js";
import {
  ScriptedConversation,
  ScriptedStation,
  turn,
} from "../../../agent/tests/station-harness.js";
import {
  type Run,
  type RunEvent,
  runRequestSchema,
} from "../../shared/run-api.js";
import {
  OUTPUT_LINE_LIMIT,
  RunConflictError,
  RunManager,
  type RunStore,
} from "../src/run-manager.js";

function memoryStore(
  initial: Run[] = [],
): RunStore & { saved: Map<string, Run> } {
  const saved = new Map(initial.map((run) => [run.id, run]));
  return {
    saved,
    appendOutput: async () => {},
    readOutput: async () => "",
    loadAll: async () => [...saved.values()],
    save: async (run) => {
      saved.set(run.id, run);
    },
  };
}

const request = runRequestSchema.parse({
  arm: "follower",
  task: "stack",
  settings: { provider: "openai", model: "gpt-6.2-astra" },
});
let station: ScriptedStation;
let store: ReturnType<typeof memoryStore>;
let time: number;

let conversations: ModelConversation[];

function manager(
  options: { environment?: NodeJS.ProcessEnv; store?: RunStore } = {},
) {
  let count = 0;
  return new RunManager({
    stationUrl: "http://station",
    mediaUrl: "http://media",
    environment: options.environment ?? { OPENAI_API_KEY: "key" },
    now: () => (time += 1),
    store: options.store ?? store,
    newId: () => `run-${++count}`,
    openConversation: () => {
      const conversation = conversations.shift();
      if (conversation === undefined) throw new Error("No scripted model");
      return conversation;
    },
  });
}

async function untilEnded(runs: RunManager, id: string): Promise<Run> {
  await station.run(
    new Promise<void>((resolve) => {
      const unsubscribe = runs.subscribe(id, (event) => {
        if (event.type === "phase" && event.phase.state === "ended") {
          unsubscribe();
          resolve();
        }
      });
    }),
  );
  return runs.get(id);
}

beforeEach(() => {
  station = new ScriptedStation();
  station.install();
  store = memoryStore();
  time = 1000;
  conversations = [];
});

afterEach(() => {
  vi.unstubAllGlobals();
});

test("a completed run is recorded with its output", async () => {
  conversations.push(
    new ScriptedConversation([turn(["finish", { reason: "all done" }])]),
  );
  const runs = manager();
  const started = runs.start(request);
  const events: RunEvent[] = [];
  runs.subscribe(started.id, (event) => events.push(event));
  expect(started.phase.state).toBe("running");
  const ended = await untilEnded(runs, started.id);
  expect(ended.phase).toMatchObject({
    state: "ended",
    outcome: { kind: "completed" },
    response: "all done",
  });
  expect(ended.output).toContain("all done");
  expect(events).toContainEqual({ type: "phase", phase: ended.phase });
  expect(store.saved.get(started.id)?.phase).toMatchObject({
    state: "ended",
    response: "all done",
  });
  expect(store.saved.get(started.id)?.request).toEqual(started.request);
});

test("a successful metadata save clears a transient disk error", async () => {
  const loggedError = vi.spyOn(console, "error").mockImplementation(() => {});
  try {
    let saveAttempts = 0;
    const recoveringStore: RunStore = {
      ...store,
      async save(run) {
        if (saveAttempts++ === 0)
          throw new Error("disk temporarily unavailable");
        await store.save(run);
      },
    };
    conversations.push(
      new ScriptedConversation([turn(["finish", { reason: "done" }])]),
    );
    const runs = manager({ store: recoveringStore });
    const started = runs.start(request);
    const events: RunEvent[] = [];
    runs.subscribe(started.id, (event) => events.push(event));
    const ended = await untilEnded(runs, started.id);
    expect(
      events.filter((event) => event.type === "persistence_error"),
    ).toEqual([
      { type: "persistence_error", detail: "disk temporarily unavailable" },
      { type: "persistence_error", detail: null },
    ]);
    expect(ended.persistenceError).toBeUndefined();
    expect(store.saved.get(started.id)?.persistenceError).toBeUndefined();
  } finally {
    loggedError.mockRestore();
  }
});

test("explicit model settings are preserved in the run record", async () => {
  conversations.push(
    new ScriptedConversation([turn(["finish", { reason: "done" }])]),
  );
  const runs = manager();
  const explicitRequest = runRequestSchema.parse({
    ...request,
    settings: {
      provider: "openai",
      model: "custom-model",
      reasoningEffort: "low",
      inferenceMode: "fast",
    },
  });
  const started = runs.start(explicitRequest);
  expect(started.request).toEqual(explicitRequest);
  await untilEnded(runs, started.id);
});

test("stop aborts the loop, stops the arm and records the run stopped", async () => {
  const model = new ScriptedConversation([
    () => {
      runs.stop("run-1");
      return turn(["stop", {}]);
    },
  ]);
  conversations.push(model);
  const runs = manager();
  const run = runs.start(request);
  const ended = await untilEnded(runs, run.id);
  expect(ended.phase).toMatchObject({ outcome: { kind: "stopped" } });
  expect(station.leaseStops).toContain("follower");
  expect(station.returned).toBe(true);
});

test("a loop that throws is a failed run and the manager keeps working", async () => {
  conversations.push(new ScriptedConversation([new Error("model exploded")]));
  conversations.push(
    new ScriptedConversation([turn(["finish", { reason: "ok" }])]),
  );
  const runs = manager();
  const failed = await untilEnded(runs, runs.start(request).id);
  expect(failed.phase).toMatchObject({
    outcome: { kind: "failed", detail: "model exploded" },
  });
  const next = await untilEnded(runs, runs.start(request).id);
  expect(next.phase).toMatchObject({ outcome: { kind: "completed" } });
});

test("a run whose lease was lost is recorded incomplete", async () => {
  conversations.push(
    new ScriptedConversation([
      () => {
        station.loseLease();
        return turn(["stop", {}]);
      },
    ]),
  );
  const runs = manager();
  const ended = await untilEnded(runs, runs.start(request).id);
  expect(ended.phase).toMatchObject({
    outcome: {
      kind: "incomplete",
      detail: expect.stringContaining("The lease was lost"),
    },
  });
});

test("a missing API key fails the run without touching the station", () => {
  const runs = manager({ environment: {} });
  const run = runs.start(request);
  expect(runs.get(run.id).phase).toMatchObject({
    outcome: {
      kind: "failed",
      detail: expect.stringContaining("OPENAI_API_KEY"),
    },
  });
  expect(station.requests).toEqual([]);
});

test("only one run is active at a time", async () => {
  conversations.push(
    new ScriptedConversation([turn(["finish", { reason: "ok" }])]),
  );
  conversations.push(
    new ScriptedConversation([turn(["finish", { reason: "ok" }])]),
  );
  const runs = manager();
  const first = runs.start(request);
  expect(() => runs.start(request)).toThrow(RunConflictError);
  await untilEnded(runs, first.id);
  const second = runs.start(request);
  await untilEnded(runs, second.id);
});

test("output keeps the last lines and reaches subscribers", async () => {
  const lines = Array.from(
    { length: OUTPUT_LINE_LIMIT + 5 },
    (_, line) => `line ${line}`,
  );
  conversations.push(
    new ScriptedConversation([turn(["finish", { reason: "x" }])]),
  );
  const runs = manager();
  const run = runs.start(request);
  const events: RunEvent[] = [];
  runs.subscribe(run.id, (event) => events.push(event));
  const ended = await untilEnded(runs, run.id);
  expect(events.some((event) => event.type === "output")).toBe(true);
  expect(ended.output.length).toBeLessThanOrEqual(OUTPUT_LINE_LIMIT);
  expect(lines.length).toBeGreaterThan(OUTPUT_LINE_LIMIT);
});

test("a run left running by a dead server is interrupted on open", async () => {
  const stale: Run = {
    id: "old",
    createdAt: 1,
    request,
    phase: { state: "running", startedAt: 5 },
    output: [],
  };
  const reopened = manager({ store: memoryStore([stale]) });
  await reopened.open();
  expect(reopened.get("old").phase).toMatchObject({
    state: "ended",
    startedAt: 5,
    outcome: { kind: "interrupted" },
  });
  expect(reopened.current()?.id).toBe("old");
});

test("shutdown aborts the active loop and records it interrupted", async () => {
  conversations.push(
    new ScriptedConversation([
      () => {
        void runs.shutdown();
        return turn(["stop", {}]);
      },
    ]),
  );
  const runs = manager();
  const run = runs.start(request);
  const ended = await untilEnded(runs, run.id);
  expect(ended.phase).toMatchObject({ outcome: { kind: "interrupted" } });
});

test("shutdown while stopping records the run as interrupted", async () => {
  conversations.push(
    new ScriptedConversation([
      () => {
        runs.stop("run-1");
        void runs.shutdown();
        return turn(["stop", {}]);
      },
    ]),
  );
  const runs = manager();
  const run = runs.start(request);
  const ended = await untilEnded(runs, run.id);
  expect(ended.phase).toMatchObject({ outcome: { kind: "interrupted" } });
  expect(station.returned).toBe(true);
});

test("history lists newest first and current prefers the active run", async () => {
  conversations.push(
    new ScriptedConversation([turn(["finish", { reason: "a" }])]),
  );
  conversations.push(
    new ScriptedConversation([turn(["finish", { reason: "b" }])]),
  );
  const runs = manager();
  const first = runs.start(request);
  await untilEnded(runs, first.id);
  const second = runs.start(request);
  expect(runs.history().map((run) => run.id)).toEqual([second.id, first.id]);
  expect(runs.current()?.id).toBe(second.id);
  await untilEnded(runs, second.id);
});

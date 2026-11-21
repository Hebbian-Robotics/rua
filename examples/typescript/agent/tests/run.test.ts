import { afterEach, beforeEach, expect, test, vi } from "vitest";
import type { LoopConfig } from "../src/loop.js";
import {
  type AgentConfig,
  apiKeyFromEnvironment,
  runAgent,
} from "../src/run.js";
import {
  ScriptedConversation,
  ScriptedStation,
  turn,
} from "./station-harness.js";

const loopConfig: LoopConfig = {
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
const config: AgentConfig = {
  ...loopConfig,
  stationUrl: "http://station",
  mediaUrl: "http://media",
  apiKey: "unused",
};

let station: ScriptedStation;

beforeEach(() => {
  station = new ScriptedStation();
  station.install();
});

afterEach(() => {
  vi.unstubAllGlobals();
});

test("a finished run is completed, with its output and usage", async () => {
  const lines: string[] = [];
  const outcome = await station.run(
    runAgent(config, {
      openConversation: () =>
        new ScriptedConversation([turn(["finish", { reason: "done" }])]),
      onEvent: (event) => lines.push(event.line),
    }),
  );
  expect(outcome).toMatchObject({
    kind: "completed",
    result: "done",
    usage: { turns: 1, input_tokens: 10 },
  });
  expect(lines.length).toBeGreaterThan(0);
});

test("a lost lease is an incomplete run, not a completed one", async () => {
  const outcome = await station.run(
    runAgent(config, {
      openConversation: () =>
        new ScriptedConversation([
          () => {
            station.loseLease();
            return turn(["stop", {}]);
          },
        ]),
    }),
  );
  expect(outcome).toMatchObject({
    kind: "incomplete",
    result: expect.stringContaining("The lease was lost"),
  });
});

test("aborting stops the arm and ends the run as stopped", async () => {
  const cancellation = new AbortController();
  const outcome = await station.run(
    runAgent(config, {
      signal: cancellation.signal,
      openConversation: () =>
        new ScriptedConversation([
          () => {
            cancellation.abort();
            return turn(["stop", {}]);
          },
        ]),
    }),
  );
  expect(outcome.kind).toBe("stopped");
  expect(station.leaseStops).toContain("follower");
  expect(station.returned).toBe(true);
});

test("a loop that throws is a failed outcome, not a rejection", async () => {
  const outcome = await station.run(
    runAgent(config, {
      openConversation: () => new ScriptedConversation([new Error("boom")]),
    }),
  );
  expect(outcome).toMatchObject({ kind: "failed", reason: "boom" });
});

test("a missing API key names the variable and never a value", () => {
  expect(() => apiKeyFromEnvironment("openai", {})).toThrow("OPENAI_API_KEY");
  expect(apiKeyFromEnvironment("openai", { OPENAI_API_KEY: "k" })).toBe("k");
});

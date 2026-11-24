// @vitest-environment jsdom

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import type { Run } from "../shared/run-api.js";
import { agentApi } from "../src/agent-api.js";
import { useAgentRun } from "../src/features/agent/use-agent-run.js";

const baseUrl = "http://agent.test";
const run: Run = {
  id: "active-run",
  createdAt: 1000,
  request: {
    arm: "follower",
    task: "stack the cups",
    cameras: [],
    optionalCameras: [],
    settings: {
      provider: "openai",
      model: "gpt-6.2-astra",
      reasoningEffort: null,
      inferenceMode: null,
    },
  },
  phase: { state: "running", startedAt: 1000 },
  output: [],
};

class TestEventSource extends EventTarget {
  static opened: TestEventSource[] = [];
  close = vi.fn();

  constructor(readonly url: string) {
    super();
    TestEventSource.opened.push(this);
  }
}

function AgentRunProbe() {
  useAgentRun(baseUrl);
  return null;
}

let root: Root | undefined;
let queryClient: QueryClient;

beforeEach(() => {
  vi.useFakeTimers();
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  vi.stubGlobal("EventSource", TestEventSource);
  TestEventSource.opened = [];
  queryClient = new QueryClient({
    defaultOptions: { queries: { gcTime: Infinity, staleTime: Infinity } },
  });
  queryClient.setQueryData(["agent-run", baseUrl], run);
  vi.spyOn(agentApi, "providers").mockResolvedValue([]);
});

afterEach(async () => {
  await unmountProbe();
  queryClient.clear();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

async function mountProbe() {
  const mounted = createRoot(document.createElement("div"));
  root = mounted;
  await act(async () => {
    mounted.render(
      createElement(
        QueryClientProvider,
        { client: queryClient },
        createElement(AgentRunProbe),
      ),
    );
  });
  const [source] = TestEventSource.opened;
  if (!source)
    throw new Error("the running agent should open its event stream");
  return source;
}

async function unmountProbe() {
  if (root === undefined) return;
  const mounted = root;
  root = undefined;
  await act(async () => mounted.unmount());
}

async function advance(milliseconds: number) {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(milliseconds);
  });
}

test("a silent stream falls back to polling and reconnects after a successful poll", async () => {
  const poll = vi
    .spyOn(agentApi, "currentRun")
    .mockRejectedValueOnce(new Error("the agent is still unavailable"))
    .mockResolvedValue(run);
  const first = await mountProbe();
  await advance(29_999);
  expect(first.close).not.toHaveBeenCalled();
  expect(poll).not.toHaveBeenCalled();

  await advance(1);
  expect(first.close).toHaveBeenCalled();
  expect(TestEventSource.opened).toHaveLength(1);

  await advance(3_001);
  expect(poll).toHaveBeenCalledExactlyOnceWith(baseUrl);
  expect(TestEventSource.opened).toHaveLength(1);

  await advance(3_001);
  expect(poll).toHaveBeenCalledTimes(2);
  expect(TestEventSource.opened.map((source) => source.url)).toEqual([
    "http://agent.test/api/runs/active-run/events",
    "http://agent.test/api/runs/active-run/events",
  ]);
});

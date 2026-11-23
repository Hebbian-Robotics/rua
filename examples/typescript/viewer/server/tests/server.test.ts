import { afterEach, expect, test, vi } from "vitest";
import { ScriptedStation } from "../../../agent/tests/station-harness.js";
import { runSchema } from "../../shared/run-api.js";
import { allowedViewerOrigins } from "../src/origins.js";
import { RunManager } from "../src/run-manager.js";
import { createApp } from "../src/server.js";

afterEach(() => {
  vi.unstubAllGlobals();
});

function app() {
  new ScriptedStation().install();
  const manager = new RunManager({
    stationUrl: "http://station",
    mediaUrl: "http://media",
    environment: { OPENAI_API_KEY: "key" },
    openConversation: () => ({
      nextTurn: () => new Promise(() => {}),
      addToolResults() {},
      addObservation() {},
    }),
    now: () => 1,
    store: {
      loadAll: async () => [],
      save: async () => {},
      appendOutput: async () => {},
      readOutput: async () => "",
    },
    newId: () => "run-1",
  });
  return createApp({
    manager,
    allowedOrigins: ["http://station:8792"],
    configuredProviders: () => ["openai"],
  });
}

const body = JSON.stringify({
  arm: "follower",
  task: "t",
  settings: { provider: "openai" },
});
const post = {
  method: "POST",
  headers: { "Content-Type": "application/json" },
  body,
};

test("a run is created and started in one call, then conflicts", async () => {
  const server = app();
  expect((await server.request("/api/runs/current")).status).toBe(200);
  expect(await (await server.request("/api/runs/current")).json()).toBeNull();
  const created = await server.request("/api/runs", post);
  expect(created.status).toBe(201);
  expect(runSchema.parse(await created.json()).phase.state).toBe("running");
  const second = await server.request("/api/runs", post);
  console.log(
    JSON.stringify(await (await server.request("/api/runs/current")).json()),
  );
  expect(second.status).toBe(409);
  const stopped = runSchema.parse(
    await (
      await server.request("/api/runs/run-1/stop", { method: "POST" })
    ).json(),
  );
  expect(stopped.phase.state).toBe("stopping");
});

test("bad bodies and unknown runs are refused with a detail", async () => {
  const server = app();
  expect(
    (await server.request("/api/runs", { ...post, body: "{}" })).status,
  ).toBe(400);
  expect(
    (await server.request("/api/runs", { ...post, body: "nope" })).status,
  ).toBe(400);
  const unsupported = await server.request("/api/runs", {
    ...post,
    body: JSON.stringify({
      arm: "follower",
      task: "t",
      settings: {
        provider: "gemini",
        model: "gemini-4-carbon",
        reasoningEffort: "xhigh",
      },
    }),
  });
  expect(unsupported.status).toBe(400);
  expect(await unsupported.json()).toEqual({
    detail: expect.stringContaining("gemini-4-carbon"),
  });
  expect(await (await server.request("/api/runs/current")).json()).toBeNull();
  const missing = await server.request("/api/runs/ghost");
  expect(missing.status).toBe(404);
  expect(await missing.json()).toEqual({ detail: "No run ghost" });
});

test("CORS allows the viewer origin only", async () => {
  const server = app();
  const allowed = await server.request("/api/health", {
    headers: { Origin: "http://station:8792" },
  });
  expect(allowed.headers.get("access-control-allow-origin")).toBe(
    "http://station:8792",
  );
  const other = await server.request("/api/health", {
    headers: { Origin: "http://evil" },
  });
  expect(other.headers.get("access-control-allow-origin")).toBeNull();
  expect(await (await server.request("/api/model-providers")).json()).toEqual([
    { provider: "openai", defaultModel: "gpt-6.2-astra" },
  ]);
});

test("the default origins include the configured viewer, and an override replaces them", () => {
  expect(
    allowedViewerOrigins("http://127.0.0.1:8795/path", undefined),
  ).toContain("http://127.0.0.1:8795");
  expect(allowedViewerOrigins("http://127.0.0.1:8795", "http://other")).toEqual(
    ["http://other"],
  );
});

test("an idle run event stream sends a heartbeat", async () => {
  const server = app();
  expect((await server.request("/api/runs", post)).status).toBe(201);
  vi.useFakeTimers();
  try {
    const response = await server.request("/api/runs/run-1/events");
    const reader = response.body?.getReader();
    if (!reader) throw new Error("Missing run event stream");
    try {
      const decoder = new TextDecoder();
      const snapshot = await reader.read();
      expect(decoder.decode(snapshot.value)).toContain('"type":"snapshot"');
      const heartbeat = reader.read();
      await vi.advanceTimersByTimeAsync(10_000);
      expect(decoder.decode((await heartbeat).value)).toContain(
        "event: heartbeat",
      );
    } finally {
      await reader.cancel();
    }
  } finally {
    vi.useRealTimers();
  }
});

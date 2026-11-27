import { afterEach, expect, test, vi } from "vitest";
import { ScriptedStation } from "../../../agent/tests/station-harness.js";
import {
  MAX_RUN_REQUEST_BYTES,
  modelProvidersSchema,
  runSchema,
} from "../../shared/run-api.js";
import { allowedViewerOrigins } from "../src/origins.js";
import { RunManager } from "../src/run-manager.js";
import { createApp } from "../src/server.js";

afterEach(() => {
  vi.unstubAllGlobals();
});

function app(environment: NodeJS.ProcessEnv = { OPENAI_API_KEY: "key" }) {
  new ScriptedStation().install();
  const manager = new RunManager({
    stationUrl: "http://station",
    environment,
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
    configuredProviders: environment["OPENAI_API_KEY"] ? ["openai"] : [],
  });
}

const body = JSON.stringify({
  arm: "follower",
  task: "t",
  settings: { provider: "openai", model: "gpt-6.2-astra" },
});
const post = {
  method: "POST",
  headers: { "Content-Type": "application/json" },
  body,
};

test("a keyless viewer accepts a browser key only for the submitted run", async () => {
  const server = app({});
  expect(await (await server.request("/api/health")).json()).toEqual({
    ok: true,
  });
  expect((await server.request("/api/runs", post)).status).toBe(400);
  const response = await server.request("/api/runs", {
    ...post,
    headers: { ...post.headers, "X-Model-Api-Key": "browser-secret" },
  });
  expect(response.status).toBe(201);
  expect(await response.text()).not.toContain("browser-secret");
  expect(await (await server.request("/api/runs")).text()).not.toContain(
    "browser-secret",
  );
  expect(await (await server.request("/api/model-providers")).json()).toEqual(
    expect.arrayContaining([
      expect.objectContaining({ provider: "openai", available: false }),
    ]),
  );
});

test("a run is created and started in one call, then conflicts", async () => {
  const server = app();
  expect((await server.request("/api/runs/current")).status).toBe(200);
  expect(await (await server.request("/api/runs/current")).json()).toBeNull();
  const created = await server.request("/api/runs", post);
  expect(created.status).toBe(201);
  expect(runSchema.parse(await created.json()).phase.state).toBe("running");
  const second = await server.request("/api/runs", post);
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
  for (const settings of [{ model: "gpt-6.2-astra" }, { provider: "openai" }]) {
    const response = await server.request("/api/runs", {
      ...post,
      body: JSON.stringify({ arm: "follower", task: "t", settings }),
    });
    expect(response.status).toBe(400);
  }
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
  const noUltrafast = await server.request("/api/runs", {
    ...post,
    body: JSON.stringify({
      arm: "follower",
      task: "t",
      settings: {
        provider: "anthropic",
        model: "claude-fable-5-2",
        inferenceMode: "ultrafast",
      },
    }),
  });
  expect(noUltrafast.status).toBe(400);
  expect(await noUltrafast.json()).toEqual({
    detail: expect.stringContaining("not offered by anthropic"),
  });
  expect(await (await server.request("/api/runs/current")).json()).toBeNull();
  const missing = await server.request("/api/runs/ghost");
  expect(missing.status).toBe(404);
  expect(await missing.json()).toEqual({ detail: "No run ghost" });
});

test("invalid image content and oversized run uploads are rejected before a run starts", async () => {
  const server = app();
  const invalid = await server.request("/api/runs", {
    ...post,
    body: JSON.stringify({
      ...JSON.parse(body),
      promptImages: [{ filename: "reference.jpg", jpegBase64: "aGVsbG8=" }],
    }),
  });
  expect(invalid.status).toBe(400);
  const oversized = await server.request("/api/runs", {
    ...post,
    body: " ".repeat(MAX_RUN_REQUEST_BYTES + 128 * 1024),
  });
  expect(oversized.status).toBe(413);
  expect(await (await server.request("/api/runs/current")).json()).toBeNull();
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
});

test("the provider menu is ordered and includes availability and model choices", async () => {
  const response = await app().request("/api/model-providers");
  expect(response.status).toBe(200);
  const providers = modelProvidersSchema.parse(await response.json());
  expect(
    providers.map(({ provider, available }) => [provider, available]),
  ).toEqual([
    ["openai", true],
    ["gemini", false],
    ["anthropic", false],
  ]);
  for (const { models } of providers) {
    expect(models.length).toBeGreaterThan(0);
  }
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

import { spawnSync } from "node:child_process";
import { afterEach, expect, test, vi } from "vitest";
import { ScriptedStation } from "../../../agent/tests/station-harness.js";
import {
  MAX_PROMPT_IMAGE_BASE64_LENGTH,
  runSchema,
} from "../../shared/run-api.js";
import { allowedViewerOrigins } from "../src/origins.js";
import { RunManager } from "../src/run-manager.js";
import { createApp } from "../src/server.js";

afterEach(() => {
  vi.unstubAllGlobals();
});

function app(manualOnly = false) {
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
    manualOnly,
    configuredProviders: ["openai"],
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

test("viewer startup requires a model-provider key by default", () => {
  const result = spawnSync(
    process.execPath,
    [
      "--import",
      "tsx",
      "--conditions=development",
      "examples/typescript/viewer/server/src/main.ts",
    ],
    {
      env: {
        ...process.env,
        OPENAI_API_KEY: "",
        ANTHROPIC_API_KEY: " ",
        GEMINI_API_KEY: "",
        GOOGLE_API_KEY: "",
        AGENT_PORT: "0",
      },
      encoding: "utf-8",
      timeout: 5_000,
    },
  );
  expect(result.status, result.stderr).toBe(1);
  expect(result.stderr).toContain("OPENAI_API_KEY");
  expect(result.stderr).toContain("--manual-control-only-no-agent");
});

test("manual-only mode refuses agent runs even with a configured provider", async () => {
  const server = app(true);
  expect(await (await server.request("/api/health")).json()).toEqual({
    ok: true,
    manualOnly: true,
  });
  expect((await server.request("/api/runs", post)).status).toBe(403);
  expect(await (await server.request("/api/runs/current")).json()).toBeNull();
});

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
      promptImage: { filename: "reference.jpg", jpegBase64: "aGVsbG8=" },
    }),
  });
  expect(invalid.status).toBe(400);
  const oversized = await server.request("/api/runs", {
    ...post,
    body: " ".repeat(MAX_PROMPT_IMAGE_BASE64_LENGTH + 128 * 1024),
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
  expect(await response.json()).toMatchObject([
    {
      provider: "openai",
      available: true,
      models: [{ id: "gpt-6.2-astra" }, { id: "gpt-6-astra" }],
    },
    {
      provider: "gemini",
      available: false,
      models: [
        { id: "gemini-4-neon" },
        { id: "gemini-4-hydrogen" },
        { id: "gemini-4-carbon" },
        { id: "gemini-4-argon" },
      ],
    },
    {
      provider: "anthropic",
      available: false,
      models: [{ id: "claude-fable-5-2" }, { id: "claude-opus-5-5" }],
    },
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

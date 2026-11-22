import { randomUUID } from "node:crypto";
import { serve } from "@hono/node-server";
import type { Provider } from "../../shared/run-api.js";
import { allowedViewerOrigins } from "./origins.js";
import { RunManager } from "./run-manager.js";
import { fileRunStore } from "./run-store.js";
import { createApp } from "./server.js";

// Model keys may live in a .env file in the working directory; exported variables win.
try {
  process.loadEnvFile();
} catch {
  // No .env file.
}

const environment = process.env;
const port = Number(environment["AGENT_PORT"] ?? 8793);
const host = environment["AGENT_HOST"] ?? "127.0.0.1";
const stationUrl =
  environment["HEBBIAN_STATION_URL"] ?? "http://127.0.0.1:8792";
const allowedOrigins = allowedViewerOrigins(
  stationUrl,
  environment["AGENT_ALLOWED_ORIGINS"],
);

const providerKeys: [Provider, string][] = [
  ["openai", "OPENAI_API_KEY"],
  ["gemini", "GEMINI_API_KEY"],
  ["anthropic", "ANTHROPIC_API_KEY"],
];

const manager = new RunManager({
  stationUrl,
  mediaUrl: environment["HEBBIAN_MEDIA_SERVER"] ?? stationUrl,
  environment,
  now: Date.now,
  store: fileRunStore(environment["AGENT_RUNS_DIR"] ?? ".agent-runs"),
  newId: randomUUID,
});
await manager.open();

const app = createApp({
  manager,
  allowedOrigins,
  // Report only whether a key exists, never its value.
  configuredProviders: () =>
    providerKeys
      .filter(([, variable]) => (environment[variable] ?? "").trim() !== "")
      .map(([provider]) => provider),
});
const server = serve({ fetch: app.fetch, hostname: host, port }, (address) =>
  console.log(
    `Agent server on http://${host}:${address.port}, station ${stationUrl}`,
  ),
);

let shuttingDown = false;
for (const signal of ["SIGINT", "SIGTERM"] as const) {
  process.on(signal, () => {
    if (shuttingDown) return;
    shuttingDown = true;
    server.close();
    void manager.shutdown().finally(() => process.exit(0));
  });
}

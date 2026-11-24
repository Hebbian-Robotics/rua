import { randomUUID } from "node:crypto";
import { resolve } from "node:path";
import { parseArgs } from "node:util";
import { serve } from "@hono/node-server";
import { serveStatic } from "@hono/node-server/serve-static";
import { createRecordingClient } from "hebbian-rua";
import type { Context } from "hono";
import { csrf } from "hono/csrf";
import { proxy } from "hono/proxy";
import {
  API_KEY_VARIABLES,
  PROVIDERS,
} from "../../../agent/src/providers/types.js";
import { configuredApiKey } from "../../../agent/src/run.js";
import { manualSessions } from "./manual-sessions.js";
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

const { values } = parseArgs({
  options: { "manual-only": { type: "boolean", default: false } },
});
const manualOnly = values["manual-only"];
const environment = process.env;
const configuredProviders = PROVIDERS.filter(
  (provider) => configuredApiKey(provider, environment) !== undefined,
);
if (!manualOnly && configuredProviders.length === 0) {
  console.error(
    `Set one of ${Object.values(API_KEY_VARIABLES).join(", ")} in the environment or .env to start the viewer. For manual control only, run pnpm start:viewer --manual-only.`,
  );
  process.exit(1);
}
const port = Number(environment["AGENT_PORT"] ?? 8793);
const host = environment["AGENT_HOST"] ?? "127.0.0.1";
const stationUrl =
  environment["HEBBIAN_STATION_URL"] ?? "http://127.0.0.1:8792";
const allowedOrigins = allowedViewerOrigins(
  `http://${host}:${port}`,
  environment["AGENT_ALLOWED_ORIGINS"],
);

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
  manualOnly,
  configuredProviders,
});
const recording = createRecordingClient(stationUrl);
app.get("/api/recording/status", async (context) =>
  context.json(await recording.status()),
);
const protectRecordingWrite = csrf({
  origin: (origin, context) =>
    origin === new URL(context.req.url).origin ||
    allowedOrigins.includes(origin),
});
app.post("/api/recording/start", protectRecordingWrite, async (context) =>
  context.json(await recording.start()),
);
app.post("/api/recording/stop", protectRecordingWrite, async (context) => {
  await recording.stop();
  return context.json(null);
});
app.route("/api/manual", manualSessions(stationUrl));
app.get("/", (context) => context.redirect("/viewer/"));
app.get("/viewer", (context) => context.redirect("/viewer/"));
app.get("/api/control/capabilities", forwardStationApi);
app.get("/api/controller", forwardStationApi);
app.post("/api/controller/reconnect", forwardStationApi);
app.get("/api/media/status", forwardStationApi);
app.get("/api/media/access-token", forwardStationApi);
app.post("/api/media/phone-invites", forwardStationApi);
app.get("/api/recording/enabled", forwardStationApi);
app.get("/api/recordings", forwardStationApi);
app.on(["GET", "HEAD", "DELETE"], "/api/recordings/*", forwardStationApi);
app.use(
  "/viewer/*",
  serveStatic({
    root: resolve(import.meta.dirname, "../../../../../dist/viewer"),
    rewriteRequestPath: (path) => path.slice("/viewer".length),
  }),
);
app.get("/model/*", (context) =>
  forwardStationRequest(context, new URL(context.req.url).pathname),
);

function forwardStationApi(context: Context): Promise<Response> {
  const path = new URL(context.req.url).pathname.slice("/api".length);
  return forwardStationRequest(context, path);
}

async function forwardStationRequest(
  context: Context,
  path: string,
): Promise<Response> {
  const incoming = new URL(context.req.url);
  const target = new URL(stationUrl);
  target.pathname = path;
  target.search = incoming.search;
  const headers = new Headers();
  for (const name of [
    "accept",
    "content-type",
    "range",
    "if-range",
    "if-none-match",
    "if-modified-since",
  ]) {
    const value = context.req.header(name);
    if (value !== undefined) headers.set(name, value);
  }
  return proxy(target, { raw: context.req.raw, headers, redirect: "manual" });
}

const server = serve({ fetch: app.fetch, hostname: host, port }, (address) =>
  console.log(
    `Viewer on http://${host}:${address.port}, station ${stationUrl}`,
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

import { StationHttpError } from "hebbian-rua";
import { Hono } from "hono";
import { bodyLimit } from "hono/body-limit";
import { cors } from "hono/cors";
import { HTTPException } from "hono/http-exception";
import { streamSSE } from "hono/streaming";
import type { ZodType } from "zod";
import { PROVIDERS } from "../../../agent/src/providers/types.js";
import {
  MAX_PROMPT_IMAGE_BASE64_LENGTH,
  type Provider,
  RUN_STREAM_HEARTBEAT_EVENT,
  type RunEvent,
  runRequestSchema,
} from "../../shared/run-api.js";
import { viewerModelCatalog } from "./model-catalog.js";
import {
  RunConflictError,
  type RunManager,
  RunNotFoundError,
  RunSettingsError,
} from "./run-manager.js";

const RUN_STREAM_HEARTBEAT_MILLISECONDS = 10_000;

export interface AppOptions {
  manager: RunManager;
  allowedOrigins: string[];
  manualOnly: boolean;
  configuredProviders: readonly Provider[];
}

export function createApp({
  manager,
  allowedOrigins,
  manualOnly,
  configuredProviders,
}: AppOptions): Hono {
  const app = new Hono();
  app.use("/api/*", cors({ origin: allowedOrigins }));
  app.onError((error, context) => {
    if (error instanceof HTTPException) return error.getResponse();
    if (error instanceof StationHttpError)
      return Response.json({ detail: error.message }, { status: error.status });
    if (error instanceof RunConflictError)
      return context.json({ detail: error.message }, 409);
    if (error instanceof RunNotFoundError)
      return context.json({ detail: error.message }, 404);
    if (error instanceof RunSettingsError)
      return context.json({ detail: error.message }, 400);
    console.error(error);
    return context.json({ detail: error.message }, 500);
  });

  app.get("/api/health", (context) => context.json({ ok: true, manualOnly }));
  app.get("/api/model-providers", (context) =>
    context.json(
      manualOnly
        ? []
        : PROVIDERS.map((provider) => ({
            provider,
            ...viewerModelCatalog[provider],
            available: configuredProviders.includes(provider),
          })),
    ),
  );
  app.get("/api/runs/current", (context) => context.json(manager.current()));
  app.get("/api/runs", (context) => context.json(manager.history()));
  app.get("/api/runs/:id", (context) =>
    context.json(manager.get(context.req.param("id"))),
  );
  app.get("/api/runs/:id/output", async (context) => {
    const output = await manager.output(context.req.param("id"));
    return context.body(output, 200, {
      "Content-Type": "application/x-ndjson; charset=utf-8",
    });
  });
  app.post(
    "/api/runs",
    bodyLimit({
      maxSize: MAX_PROMPT_IMAGE_BASE64_LENGTH + 64 * 1024,
      onError: (context) =>
        context.json({ detail: "Run request is too large." }, 413),
    }),
    async (context) => {
      if (manualOnly)
        return context.json(
          {
            detail:
              "Agent runs are disabled: the viewer was started with --manual-control-only-no-agent.",
          },
          403,
        );
      const body = await readJson(context.req.json(), runRequestSchema);
      if (!body.success) return context.json({ detail: body.detail }, 400);
      return context.json(manager.start(body.data), 201);
    },
  );
  app.post("/api/runs/:id/stop", (context) =>
    context.json(manager.stop(context.req.param("id"))),
  );
  app.get("/api/runs/:id/events", (context) => {
    const id = context.req.param("id");
    manager.get(id); // 404 before the stream opens
    return streamSSE(context, async (stream) => {
      const queue: RunEvent[] = [];
      let wake = () => {};
      const unsubscribe = manager.subscribe(id, (event) => {
        queue.push(event);
        wake();
      });
      stream.onAbort(() => {
        unsubscribe();
        wake();
      });
      try {
        const run = manager.get(id);
        await stream.writeSSE({
          data: JSON.stringify({ type: "snapshot", run }),
        });
        let ended = run.phase.state === "ended";
        while (!ended && !stream.aborted) {
          for (const event of queue.splice(0)) {
            await stream.writeSSE({ data: JSON.stringify(event) });
            if (event.type === "phase" && event.phase.state === "ended")
              ended = true;
          }
          if (!ended && !stream.aborted && queue.length === 0) {
            const isIdle = await new Promise<boolean>((resolve) => {
              const timer = setTimeout(
                () => resolve(true),
                RUN_STREAM_HEARTBEAT_MILLISECONDS,
              );
              wake = () => {
                clearTimeout(timer);
                resolve(false);
              };
            });
            if (isIdle && !stream.aborted)
              await stream.writeSSE({
                event: RUN_STREAM_HEARTBEAT_EVENT,
                data: "",
              });
          }
        }
      } finally {
        unsubscribe();
      }
    });
  });
  return app;
}

async function readJson<T>(
  body: Promise<unknown>,
  schema: ZodType<T>,
): Promise<{ success: true; data: T } | { success: false; detail: string }> {
  let value: unknown;
  try {
    value = await body;
  } catch {
    return { success: false, detail: "Body is not JSON" };
  }
  const parsed = schema.safeParse(value);
  return parsed.success
    ? { success: true, data: parsed.data }
    : { success: false, detail: parsed.error.message };
}

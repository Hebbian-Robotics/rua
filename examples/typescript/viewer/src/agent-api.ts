import type { ZodType } from "zod";
import {
  currentRunSchema,
  errorBodySchema,
  healthSchema,
  modelProvidersSchema,
  type Run,
  type RunRequest,
  runSchema,
} from "../shared/run-api.js";

async function agentJson<T>(
  baseUrl: string,
  path: string,
  schema: ZodType<T>,
  init?: RequestInit,
): Promise<T> {
  const deadline = AbortSignal.timeout(10_000);
  const response = await fetch(`${baseUrl}${path}`, {
    cache: "no-store",
    ...init,
    signal: init?.signal ? AbortSignal.any([init.signal, deadline]) : deadline,
  });
  const text = await response.text();
  let body: unknown = null;
  try {
    body = JSON.parse(text);
  } catch {
    // WHY: Preserve a non-JSON error response below.
  }
  if (!response.ok) {
    const error = errorBodySchema.safeParse(body);
    throw new Error(
      error.success
        ? error.data.detail
        : `HTTP ${response.status}${text ? `: ${text.slice(0, 500)}` : ""}`,
    );
  }
  return schema.parse(body);
}

export const agentApi = {
  health: (baseUrl: string) => agentJson(baseUrl, "/api/health", healthSchema),
  providers: (baseUrl: string) =>
    agentJson(baseUrl, "/api/model-providers", modelProvidersSchema),
  currentRun: (baseUrl: string) =>
    agentJson(baseUrl, "/api/runs/current", currentRunSchema),
  startRun: (
    baseUrl: string,
    request: RunRequest,
    apiKey?: string,
  ): Promise<Run> =>
    agentJson(baseUrl, "/api/runs", runSchema, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        ...(apiKey ? { "X-Model-Api-Key": apiKey } : {}),
      },
      body: JSON.stringify(request),
    }),
  stopRun: (baseUrl: string, id: string): Promise<Run> =>
    agentJson(baseUrl, `/api/runs/${id}/stop`, runSchema, { method: "POST" }),
};

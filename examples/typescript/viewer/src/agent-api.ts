// The viewer's calls to the optional agent server: starting, stopping and
// listing runs. Live data comes from the robot station, never from here.

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

export function resolveAgentUrl(setting: string): string {
  const configured =
    setting.trim() ||
    (import.meta.env["VITE_AGENT_URL"] as string | undefined) ||
    `${globalThis.location.protocol}//${globalThis.location.hostname}:8793`;
  return configured.replace(/\/+$/, "");
}

async function agentJson<T>(
  baseUrl: string,
  path: string,
  schema: ZodType<T>,
  init?: RequestInit,
): Promise<T> {
  const response = await fetch(`${baseUrl}${path}`, {
    cache: "no-store",
    ...init,
  });
  const text = await response.text();
  let body: unknown = null;
  try {
    body = JSON.parse(text);
  } catch {
    // Preserve a non-JSON error response below.
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
  startRun: (baseUrl: string, request: RunRequest): Promise<Run> =>
    agentJson(baseUrl, "/api/runs", runSchema, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(request),
    }),
  stopRun: (baseUrl: string, id: string): Promise<Run> =>
    agentJson(baseUrl, `/api/runs/${id}/stop`, runSchema, { method: "POST" }),
};

// WHY: The agent loop as a library: one typed config in, one typed outcome out. The CLI (cli.ts)
// and the agent server (examples/typescript/viewer/server) both call runAgent.

import { createStationConnection } from "hebbian-rua";
import { type DriveOptions, drive, type LoopConfig } from "./loop.js";
import {
  API_KEY_VARIABLES,
  PROVIDERS,
  type Provider,
} from "./providers/types.js";
import type { UsageSnapshot } from "./usage.js";

export interface AgentConfig extends LoopConfig {
  stationUrl: string;

  mediaUrl: string;

  apiKey: string;
}

export type AgentEvent = { type: "output"; line: string };

export interface RunAgentOptions {
  signal?: AbortSignal;
  onEvent?: (event: AgentEvent) => void;

  openConversation?: DriveOptions["openConversation"];
}

export type AgentOutcome =
  | { kind: "completed"; result: string; usage: UsageSnapshot }
  | { kind: "stopped"; result: string; usage: UsageSnapshot }
  | { kind: "incomplete"; result: string; usage: UsageSnapshot }
  | { kind: "failed"; reason: string; cause: unknown };

export function apiKeyFromEnvironment(
  provider: Provider,
  environment: NodeJS.ProcessEnv,
): string {
  const apiKey = configuredApiKey(provider, environment);
  if (apiKey === undefined)
    throw new Error(
      `Set ${API_KEY_VARIABLES[provider]} in the environment or a .env file`,
    );
  return apiKey;
}

export function configuredProviders(
  environment: NodeJS.ProcessEnv,
): Provider[] {
  return PROVIDERS.filter(
    (provider) => configuredApiKey(provider, environment) !== undefined,
  );
}

export function configuredApiKey(
  provider: Provider,
  environment: NodeJS.ProcessEnv,
): string | undefined {
  const primaryKey = environment[API_KEY_VARIABLES[provider]];
  if (primaryKey?.trim()) return primaryKey;
  const googleKey = environment["GOOGLE_API_KEY"];
  return provider === "gemini" && googleKey?.trim() ? googleKey : undefined;
}

export async function runAgent(
  config: AgentConfig,
  options: RunAgentOptions = {},
): Promise<AgentOutcome> {
  const { stationUrl, mediaUrl, apiKey, ...loopConfig } = config;
  if (
    !PROVIDERS.includes(config.provider) ||
    typeof config.model !== "string" ||
    !config.model.trim()
  ) {
    const cause = new Error(
      "Pass a supported provider and a nonempty model ID",
    );
    return { kind: "failed", reason: cause.message, cause };
  }
  const connection = createStationConnection(stationUrl, "agent");
  connection.start();
  try {
    const driven = await drive(
      loopConfig,
      { connection, stationUrl, mediaUrl },
      {
        apiKey,
        print: (line) => options.onEvent?.({ type: "output", line }),
        ...(options.signal === undefined ? {} : { signal: options.signal }),
        ...(options.openConversation === undefined
          ? {}
          : { openConversation: options.openConversation }),
      },
    );
    const kind = driven.cancelled
      ? "stopped"
      : driven.incomplete
        ? "incomplete"
        : "completed";
    return { kind, result: driven.result, usage: driven.usage };
  } catch (error) {
    return {
      kind: "failed",
      reason: error instanceof Error ? error.message : String(error),
      cause: error,
    };
  } finally {
    connection.close();
  }
}

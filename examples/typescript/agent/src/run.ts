// The agent loop as a library: one typed config in, one typed outcome out. The CLI (cli.ts)
// and the agent server (examples/typescript/viewer/server) both call runAgent.

import { createStationConnection } from "@hebbian/robot-use-client";
import { type DriveOptions, drive, type LoopConfig } from "./loop.js";
import { API_KEY_VARIABLES, type Provider } from "./providers/types.js";
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
  const variable = API_KEY_VARIABLES[provider];
  const apiKey = environment[variable];
  if (!apiKey?.trim())
    throw new Error(`Set ${variable} in the environment or a .env file`);
  return apiKey;
}

export async function runAgent(
  config: AgentConfig,
  options: RunAgentOptions = {},
): Promise<AgentOutcome> {
  const { stationUrl, mediaUrl, apiKey, ...loopConfig } = config;
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

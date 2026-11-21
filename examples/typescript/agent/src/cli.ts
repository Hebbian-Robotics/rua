#!/usr/bin/env -S pnpm exec tsx --conditions=development
// Let a model perform a task with the station's arm.
//
//   pnpm --filter @hebbian/agent start --provider openai -- arm "Describe the robot state."
//
// A thin wrapper over runAgent for terminal use; the agent server calls runAgent in-process.
// API keys come from the environment or a .env file in the directory the command was run from.

import { realpathSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";
import {
  DEFAULT_STATION_URL,
  type OnInterruption,
} from "@hebbian/robot-use-client";
import { type LoopConfig, requirePositiveModelTurnLimit } from "./loop.js";
import { cameraNames } from "./observation.js";
import {
  DEFAULT_REASONING_EFFORT,
  PROVIDERS,
  type Provider,
  REASONING_EFFORTS,
  type ReasoningEffort,
  unsupportedReasoningEffort,
} from "./providers/types.js";
import { apiKeyFromEnvironment, runAgent } from "./run.js";

export const USAGE = `Usage: hebbian-agent --provider NAME [options] [--] <arm> <task>

  --server URL            The station's HTTP URL (default $HEBBIAN_STATION_URL or ${DEFAULT_STATION_URL})
  --media-server URL      Direct camera service (default $HEBBIAN_MEDIA_SERVER or the station URL)
  --camera NAMES          Comma-separated cameras to show the model; omitted sends state only
  --optional-camera NAMES Comma-separated cameras that may be unavailable
  --provider NAME         ${PROVIDERS.join(", ")} (required)
  --model ID              Model id; defaults to the provider's flagship
  --max-turns COUNT       Optional maximum number of model turns
  --reasoning-effort LVL  ${REASONING_EFFORTS.join(", ")} (default ${DEFAULT_REASONING_EFFORT}); most Gemini models refuse xhigh
  --on-interruption WHAT  wait_for_approval (default) or return_lease when the arm needs recovery
  --record                Record this run
  -h, --help              Print this usage`;

function oneOf<Choice extends string>(
  name: string,
  value: string,
  choices: readonly Choice[],
): Choice {
  const choice = choices.find((candidate) => candidate === value);
  if (choice === undefined)
    throw new Error(
      `--${name} must be one of ${choices.join(", ")}, not ${value}`,
    );
  return choice;
}

export interface CommandLine {
  config: LoopConfig;
  server: string;
  mediaServer: string;
}

export function parseCommandLine(
  argv: string[],
  environment: NodeJS.ProcessEnv = process.env,
): CommandLine | "help" {
  const { values, positionals } = parseArgs({
    args: argv,
    allowPositionals: true,
    options: {
      server: { type: "string" },
      "media-server": { type: "string" },
      camera: { type: "string" },
      "optional-camera": { type: "string" },
      provider: { type: "string" },
      model: { type: "string" },
      "max-turns": { type: "string" },
      "reasoning-effort": { type: "string" },
      "on-interruption": { type: "string" },
      record: { type: "boolean" },
      help: { type: "boolean", short: "h" },
    },
  });
  if (values.help) return "help";
  const [arm, task, ...extra] = positionals;
  if (arm === undefined || task === undefined || extra.length > 0)
    throw new Error("Pass exactly <arm> and <task>");
  if (values.provider === undefined) throw new Error("Pass --provider NAME");
  const required = cameraNames(values.camera);
  const optional = cameraNames(values["optional-camera"]);
  if (required.some((name) => optional.includes(name)))
    throw new Error("Camera names must be unique");
  const provider = oneOf<Provider>("provider", values.provider, PROVIDERS);
  const model = values.model ?? null;
  const maxModelTurns =
    values["max-turns"] === undefined
      ? undefined
      : requirePositiveModelTurnLimit(Number(values["max-turns"]));
  const reasoningEffort = oneOf<ReasoningEffort>(
    "reasoning-effort",
    values["reasoning-effort"] ?? DEFAULT_REASONING_EFFORT,
    REASONING_EFFORTS,
  );
  const effortProblem = unsupportedReasoningEffort(
    provider,
    model,
    reasoningEffort,
  );
  if (effortProblem !== null) throw new Error(effortProblem);
  const server =
    values.server ?? environment["HEBBIAN_STATION_URL"] ?? DEFAULT_STATION_URL;
  return {
    server,
    mediaServer:
      values["media-server"] ?? environment["HEBBIAN_MEDIA_SERVER"] ?? server,
    config: {
      arm,
      task,
      cameras: required,
      optionalCameras: optional,
      provider,
      model,
      ...(maxModelTurns === undefined ? {} : { maxModelTurns }),
      reasoningEffort,
      onInterruption: oneOf<OnInterruption>(
        "on-interruption",
        values["on-interruption"] ?? "wait_for_approval",
        ["wait_for_approval", "return_lease"],
      ),
      record: values.record ?? false,
    },
  };
}

// CONSTRAINT: `pnpm --filter` runs the script in the package directory; INIT_CWD is where pnpm was invoked.
function loadDotEnvironment(): void {
  const invocationDirectory = process.env["INIT_CWD"] ?? process.cwd();
  try {
    // Does not replace variables that are already exported.
    process.loadEnvFile(join(invocationDirectory, ".env"));
  } catch {
    // No .env file in the directory the command was run from.
  }
}

async function main(): Promise<number> {
  loadDotEnvironment();
  const commandLine = parseCommandLine(process.argv.slice(2));
  if (commandLine === "help") {
    console.log(USAGE);
    return 0;
  }
  const { config, server, mediaServer } = commandLine;
  const apiKey = apiKeyFromEnvironment(config.provider, process.env);
  const cancellation = new AbortController();
  for (const signal of ["SIGINT", "SIGTERM"] as const)
    process.once(signal, () => {
      cancellation.abort();
      // A second signal ends the process without waiting.
      process.once(signal, () => process.exit(1));
    });
  const outcome = await runAgent(
    { ...config, stationUrl: server, mediaUrl: mediaServer, apiKey },
    {
      signal: cancellation.signal,
      onEvent: (event) => console.log(event.line),
    },
  );
  if (outcome.kind === "failed") {
    console.error(outcome.reason);
    return 1;
  }
  if (outcome.kind === "incomplete") {
    console.error(`Run incomplete: ${outcome.result}`);
    return 1;
  }
  console.log(outcome.result);
  return 0;
}

const invokedPath = process.argv[1];
if (
  invokedPath !== undefined &&
  fileURLToPath(import.meta.url) === realpathSync(invokedPath)
) {
  main().then(
    (code) => process.exit(code),
    (error: unknown) => {
      console.error(error instanceof Error ? error.message : error);
      process.exit(1);
    },
  );
}

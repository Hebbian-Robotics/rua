#!/usr/bin/env -S pnpm exec tsx --conditions=development
// Let a model perform a task with the station's arm.
//
//   pnpm --filter @hebbian/agent start --provider openai -- "Describe the robot state."
//
// A thin wrapper over runAgent for terminal use; the agent server calls runAgent in-process.
// API keys come from the environment or a .env file in the directory the command was run from.

import { realpathSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { Command, CommanderError, Option } from "commander";
import { DEFAULT_STATION_URL, type OnInterruption } from "hebbian-rua";
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
import {
  apiKeyFromEnvironment,
  configuredProviders,
  PROVIDER_PRIORITY,
  runAgent,
} from "./run.js";

function createCommand(): Command {
  return new Command("hebbian-agent")
    .usage("[options] [--] [<arm>] <task>")
    .allowExcessArguments()
    .option("--arm <name>", "Arm name. Defaults to the station's sole arm.")
    .option(
      "--server <url>",
      `Station URL. Defaults to HEBBIAN_STATION_URL or ${DEFAULT_STATION_URL}.`,
    )
    .option(
      "--media-server <url>",
      "Direct camera service. Defaults to HEBBIAN_MEDIA_SERVER or the station URL.",
    )
    .option("--camera <names>", "Comma-separated cameras to show the model.")
    .option(
      "--optional-camera <names>",
      "Comma-separated cameras that may be unavailable.",
    )
    .addOption(
      new Option(
        "--provider <name>",
        "Defaults to the first configured key in order OpenAI, Gemini, Anthropic.",
      ).choices(PROVIDERS),
    )
    .option(
      "--model <id>",
      "Defaults to the provider's *_MODEL variable or example model.",
    )
    .option("--max-turns <count>", "Maximum number of model turns.", (value) =>
      requirePositiveModelTurnLimit(Number(value)),
    )
    .addOption(
      new Option(
        "--reasoning-effort <level>",
        "Most Gemini models refuse xhigh.",
      )
        .choices(REASONING_EFFORTS)
        .default(DEFAULT_REASONING_EFFORT),
    )
    .addOption(
      new Option(
        "--on-interruption <action>",
        "What to do when the arm needs recovery.",
      )
        .choices(["wait_for_approval", "return_lease"])
        .default("wait_for_approval"),
    )
    .option("--record", "Record this run.");
}

interface CommandOptions {
  arm?: string;
  server?: string;
  mediaServer?: string;
  camera?: string;
  optionalCamera?: string;
  provider?: Provider;
  model?: string;
  maxTurns?: number;
  reasoningEffort: ReasoningEffort;
  onInterruption: OnInterruption;
  record?: boolean;
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
  const command = createCommand()
    .exitOverride()
    .configureOutput({ writeOut: () => {}, writeErr: () => {} });
  try {
    command.parse(argv, { from: "user" });
  } catch (error) {
    if (
      error instanceof CommanderError &&
      error.code === "commander.helpDisplayed"
    )
      return "help";
    throw error;
  }
  const values = command.opts<CommandOptions>();
  const [armOrTask, taskForNamedArm, ...extra] = command.args;
  if (armOrTask === undefined || extra.length > 0)
    throw new Error("Pass <task> or <arm> <task>");
  if (values.arm !== undefined && taskForNamedArm !== undefined)
    throw new Error("Choose the arm either with --arm or before the task");
  const arm =
    values.arm ?? (taskForNamedArm === undefined ? undefined : armOrTask);
  const required = cameraNames(values.camera);
  const optional = cameraNames(values.optionalCamera);
  if (required.some((name) => optional.includes(name)))
    throw new Error("Camera names must be unique");
  const provider =
    values.provider ??
    configuredProviders(environment)[0] ??
    PROVIDER_PRIORITY[0];
  const environmentModel = environment[`${provider.toUpperCase()}_MODEL`];
  const model =
    values.model ?? (environmentModel?.trim() ? environmentModel : null);
  if (model !== null && !model.trim())
    throw new Error("Pass a nonempty model ID with --model");
  const { maxTurns: maxModelTurns, reasoningEffort } = values;
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
      values.mediaServer ?? environment["HEBBIAN_MEDIA_SERVER"] ?? server,
    config: {
      ...(arm === undefined ? {} : { arm }),
      task: taskForNamedArm ?? armOrTask,
      cameras: required,
      optionalCameras: optional,
      provider,
      model,
      ...(maxModelTurns === undefined ? {} : { maxModelTurns }),
      reasoningEffort,
      onInterruption: values.onInterruption,
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
    createCommand().outputHelp();
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

// What the run panel may do and say, given the last run the agent server reported.
// Pure: no DOM, no network, tested directly.

import { match } from "ts-pattern";
import {
  type InferenceMode,
  PROVIDER_INFERENCE_MODES,
  type Provider,
  type ReasoningEffort,
  unsupportedInferenceMode,
  unsupportedReasoningEffort,
} from "../../agent/src/providers/types.js";
import {
  isRunActive,
  providerSchema,
  type Run,
  type RunRequest,
  runRequestSchema,
} from "../shared/run-api.js";

export { isRunActive, type Run, type RunRequest };

type RunState =
  | "running"
  | "stopping"
  | "completed"
  | "incomplete"
  | "failed"
  | "stopped"
  | "interrupted";

const MILLISECONDS_PER_SECOND = 1000;
// Output follows new lines unless the reader scrolled up by more than this.
const FOLLOW_OUTPUT_SLACK_PIXELS = 24;

function runState(run: Run): RunState {
  return match(run.phase)
    .with({ state: "running" }, () => "running" as const)
    .with({ state: "stopping" }, () => "stopping" as const)
    .with({ state: "ended" }, ({ outcome }) => outcome.kind)
    .exhaustive();
}

export interface RunFormValues {
  task: string;
  arm: string;
  cameras: string;
  optionalCameras: string;
  provider: string;
  model: string;
  reasoningEffort: ReasoningEffort;
  inferenceMode: InferenceMode;
}

const INFERENCE_MODE_LABELS: Record<InferenceMode, string> = {
  standard: "Standard",
  fast: "Fast",
  ultrafast: "Ultrafast",
};

// The modes a provider offers, named as that provider names them.
export function inferenceModeOptions(
  provider: Provider | undefined,
): { value: InferenceMode; label: string }[] {
  const modes: readonly InferenceMode[] = provider
    ? PROVIDER_INFERENCE_MODES[provider]
    : ["standard"];
  return modes.map((value) => ({
    value,
    label:
      provider === "gemini" && value === "fast"
        ? "Priority"
        : INFERENCE_MODE_LABELS[value],
  }));
}

export type StartDecision =
  | { kind: "allowed"; request: RunRequest }
  | { kind: "blocked"; reason: string };

export function decideStart(
  form: RunFormValues,
  run: Run | null,
  armNames: string[],
  startPending: boolean,
): StartDecision {
  if (startPending) return { kind: "blocked", reason: "Starting the run…" };
  if (run !== null && isRunActive(run))
    return {
      kind: "blocked",
      reason: `A run is ${runState(run)}: ${run.request.task}`,
    };
  if (form.task.trim() === "")
    return { kind: "blocked", reason: "Describe the task first." };
  if (!armNames.includes(form.arm))
    return {
      kind: "blocked",
      reason:
        armNames.length === 0
          ? "Waiting for the station's arms."
          : "Choose an arm.",
    };
  const request = buildRunRequest(form);
  const { provider, model } = request.settings;
  const settingsProblem =
    provider == null
      ? null
      : (unsupportedReasoningEffort(
          provider,
          model ?? null,
          form.reasoningEffort,
        ) ??
        unsupportedInferenceMode(provider, model ?? null, form.inferenceMode));
  if (settingsProblem !== null)
    return { kind: "blocked", reason: settingsProblem };
  return { kind: "allowed", request };
}

function splitNames(names: string): string[] {
  return names
    .split(",")
    .map((name) => name.trim())
    .filter((name) => name !== "");
}

export function buildRunRequest(form: RunFormValues): RunRequest {
  return {
    arm: form.arm,
    task: form.task.trim(),
    cameras: splitNames(form.cameras),
    optionalCameras: splitNames(form.optionalCameras),
    settings: {
      provider: providerSchema.safeParse(form.provider).data ?? null,
      model: form.model.trim() || null,
      reasoningEffort: form.reasoningEffort,
      inferenceMode: form.inferenceMode,
    },
  };
}

function shellQuote(argument: string): string {
  return /^[\w@%+=:,./-]+$/.test(argument)
    ? argument
    : `'${argument.replaceAll("'", `'\\''`)}'`;
}

function presentFlags(flags: [string, string | null][]): string[] {
  return flags.flatMap(([flag, value]) =>
    value === null ? [] : [flag, value],
  );
}

export function manualLoopCommand(
  request: RunRequest,
  stationUrl: string,
): string {
  const { arm, task, cameras, optionalCameras, settings } =
    runRequestSchema.parse(request);
  if (settings.provider === null)
    throw new Error("Select a provider to generate an agent command");
  const cameraFlags: [string, string | null][] = [
    ["--camera", cameras.length > 0 ? cameras.join(",") : null],
    [
      "--optional-camera",
      optionalCameras.length > 0 ? optionalCameras.join(",") : null,
    ],
  ];
  const modelFlags: [string, string | null][] = [
    ["--provider", settings.provider],
    ["--model", settings.model],
    ["--reasoning-effort", settings.reasoningEffort],
    [
      "--inference-mode",
      settings.inferenceMode === "standard" ? null : settings.inferenceMode,
    ],
  ];
  return [
    "pnpm",
    "--filter",
    "@hebbian/agent",
    "start",
    "--server",
    stationUrl,
    ...presentFlags(cameraFlags),
    ...presentFlags(modelFlags),
    "--",
    arm,
    task,
  ]
    .map(shellQuote)
    .join(" ");
}

export function formatElapsed(totalSeconds: number): string {
  const seconds = Math.max(0, Math.floor(totalSeconds));
  if (seconds < 60) return `${seconds} s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60)
    return `${minutes} min ${String(seconds % 60).padStart(2, "0")} s`;
  return `${Math.floor(minutes / 60)} h ${String(minutes % 60).padStart(2, "0")} min`;
}

function elapsedSeconds(run: Run, nowSeconds: number): number | null {
  return match(run.phase)
    .with(
      { state: "running" },
      { state: "stopping" },
      ({ startedAt }) => nowSeconds - startedAt / MILLISECONDS_PER_SECOND,
    )
    .with({ state: "ended" }, ({ startedAt, endedAt }) =>
      startedAt === null
        ? null
        : (endedAt - startedAt) / MILLISECONDS_PER_SECOND,
    )
    .exhaustive();
}

export function runStatusLine(run: Run | null, nowSeconds: number): string {
  if (run === null) return "No run yet.";
  const elapsed = elapsedSeconds(run, nowSeconds);
  const failure =
    run.phase.state === "ended" &&
    (run.phase.outcome.kind === "failed" ||
      run.phase.outcome.kind === "incomplete")
      ? run.phase.outcome
      : null;
  const detail = failure === null ? "" : ` · ${failure.detail}`;
  return `${runState(run)}${elapsed === null ? "" : ` · ${formatElapsed(elapsed)}`}${detail}`;
}

export function shouldFollowOutput(
  scrollTop: number,
  clientHeight: number,
  scrollHeight: number,
): boolean {
  return (
    scrollHeight - (scrollTop + clientHeight) <= FOLLOW_OUTPUT_SLACK_PIXELS
  );
}

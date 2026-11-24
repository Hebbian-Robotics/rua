// Owns the one active agent run: runs the loop in this process, keeps its output, aborts it
// and persists every run. The store and clock are injected so tests need no files or real time.

import { type ActorRefFrom, createActor } from "xstate";
import {
  type AgentOutcome,
  apiKeyFromEnvironment,
  DEFAULT_INFERENCE_MODE,
  DEFAULT_REASONING_EFFORT,
  type RunAgentOptions,
  runAgent,
  unsupportedInferenceMode,
  unsupportedReasoningEffort,
} from "../../../agent/src/index.js";
import type {
  ParsedRunRequest,
  Run,
  RunEvent,
  RunPhase,
} from "../../shared/run-api.js";

import { runLifecycle } from "./run-lifecycle.js";

export const OUTPUT_LINE_LIMIT = 200;

export interface RunStore {
  loadAll(): Promise<Run[]>;
  save(run: Run): Promise<void>;
  appendOutput(id: string, line: string, timestamp: number): Promise<void>;
  readOutput(id: string): Promise<string>;
}

export class RunConflictError extends Error {}
export class RunNotFoundError extends Error {}
export class RunSettingsError extends Error {}

export interface RunManagerOptions {
  stationUrl: string;
  mediaUrl: string;

  environment: NodeJS.ProcessEnv;

  now: () => number;
  store: RunStore;
  newId: () => string;

  openConversation?: RunAgentOptions["openConversation"];
}

interface LiveRun {
  run: Run;

  abort: AbortController;

  finished: Promise<void> | null;
  lifecycle: ActorRefFrom<typeof runLifecycle>;
  listeners: Set<(event: RunEvent) => void>;
  outputWriteFailed: boolean;
}

export class RunManager {
  private readonly runs = new Map<string, LiveRun>();
  private writes: Promise<void> = Promise.resolve();

  constructor(private readonly options: RunManagerOptions) {}

  async open(): Promise<void> {
    for (const saved of await this.options.store.loadAll()) {
      const run =
        saved.phase.state === "ended"
          ? saved
          : {
              ...saved,
              phase: {
                state: "ended" as const,
                startedAt: saved.phase.startedAt,
                endedAt: this.options.now(),
                outcome: { kind: "interrupted" as const },
              },
            };
      this.runs.set(run.id, this.live(run));
      if (run !== saved) this.persist(run);
    }
    await this.writes;
  }

  start(request: ParsedRunRequest): Run {
    if (this.activeRun() !== null)
      throw new RunConflictError("A run is already active");
    const { promptImage, ...historyRequest } = request;
    const { provider, model } = historyRequest.settings;
    const reasoningEffort =
      request.settings.reasoningEffort ?? DEFAULT_REASONING_EFFORT;
    const inferenceMode =
      request.settings.inferenceMode ?? DEFAULT_INFERENCE_MODE;
    const settingsProblem =
      unsupportedReasoningEffort(provider, model, reasoningEffort) ??
      unsupportedInferenceMode(provider, model, inferenceMode);
    if (settingsProblem !== null) throw new RunSettingsError(settingsProblem);
    const now = this.options.now();
    const run: Run = {
      id: this.options.newId(),
      createdAt: now,
      request: {
        ...historyRequest,
        ...(promptImage ? { attachmentFilename: promptImage.filename } : {}),
        settings: { provider, model, reasoningEffort, inferenceMode },
      },
      phase: { state: "running", startedAt: now },
      output: [],
    };
    const entry = this.live(run);
    this.runs.set(run.id, entry);
    this.persist(entry.run);
    const started = entry.run;
    entry.finished = this.execute(entry, request);
    return started;
  }

  // CONSTRAINT: Aborting stops the arm; returning the manual lease and disabling torque remain
  // the viewer owner's responsibility.
  stop(id: string): Run {
    const entry = this.entry(id);
    if (entry.run.phase.state !== "running") return entry.run;
    entry.lifecycle.send({ type: "STOP" });
    const stopping = entry.run;
    entry.abort.abort();
    return stopping;
  }

  current(): Run | null {
    return this.activeRun()?.run ?? this.history()[0] ?? null;
  }

  history(): Run[] {
    return [...this.runs.values()]
      .map((entry) => entry.run)
      .sort((first, second) => second.createdAt - first.createdAt);
  }

  get(id: string): Run {
    return this.entry(id).run;
  }

  async output(id: string): Promise<string> {
    this.entry(id);
    await this.writes;
    return this.options.store.readOutput(id);
  }

  subscribe(id: string, listener: (event: RunEvent) => void): () => void {
    const { listeners } = this.entry(id);
    listeners.add(listener);
    return () => listeners.delete(listener);
  }

  async shutdown(): Promise<void> {
    const active = this.activeRun();
    if (active !== null) {
      active.lifecycle.send({ type: "SHUTDOWN" });
      active.abort.abort();
      await active.finished;
    }
    await this.writes;
  }

  private live(run: Run): LiveRun {
    const lifecycle = createActor(runLifecycle, {
      input: { phase: run.phase },
    });
    const entry: LiveRun = {
      run,
      abort: new AbortController(),
      finished: null,
      lifecycle,
      listeners: new Set(),
      outputWriteFailed: run.persistenceError !== undefined,
    };
    lifecycle.subscribe((snapshot) => {
      if (snapshot.context.phase !== entry.run.phase)
        this.setPhase(entry, snapshot.context.phase);
    });
    lifecycle.start();
    return entry;
  }

  private entry(id: string): LiveRun {
    const entry = this.runs.get(id);
    if (entry === undefined) throw new RunNotFoundError(`No run ${id}`);
    return entry;
  }

  private activeRun(): LiveRun | null {
    return (
      [...this.runs.values()].find(
        (entry) => entry.run.phase.state !== "ended",
      ) ?? null
    );
  }

  private emit(entry: LiveRun, event: RunEvent): void {
    for (const listener of entry.listeners) listener(event);
  }

  private setPhase(entry: LiveRun, phase: RunPhase): void {
    entry.run = { ...entry.run, phase };
    this.persist(entry.run);
    if (phase.state === "ended") {
      // WHY: The terminal event closes the stream. Report final persistence first.
      void this.writes.then(() => this.emit(entry, { type: "phase", phase }));
    } else {
      this.emit(entry, { type: "phase", phase });
    }
  }

  private appendLine(entry: LiveRun, line: string): void {
    entry.run.output.push(line);
    if (entry.run.output.length > OUTPUT_LINE_LIMIT) entry.run.output.shift();
    const timestamp = this.options.now();
    this.enqueueWrite(entry.run.id, "output", () =>
      this.options.store.appendOutput(entry.run.id, line, timestamp),
    );
    this.emit(entry, { type: "output", line });
  }

  private async execute(
    entry: LiveRun,
    request: ParsedRunRequest,
  ): Promise<void> {
    const { provider, model } = request.settings;
    let outcome: AgentOutcome;
    try {
      outcome = await runAgent(
        {
          runId: entry.run.id,
          arm: request.arm,
          task: request.task,
          ...(request.promptImage
            ? {
                referenceImageJpeg: Buffer.from(
                  request.promptImage.jpegBase64,
                  "base64",
                ),
              }
            : {}),
          cameras: request.cameras,
          optionalCameras: request.optionalCameras,
          ...(request.motionLimits === undefined
            ? {}
            : { motionLimits: request.motionLimits }),
          provider,
          model,
          reasoningEffort:
            request.settings.reasoningEffort ?? DEFAULT_REASONING_EFFORT,
          inferenceMode:
            request.settings.inferenceMode ?? DEFAULT_INFERENCE_MODE,
          onInterruption: "wait_for_approval",
          record: false,
          stationUrl: this.options.stationUrl,
          mediaUrl: this.options.mediaUrl,
          apiKey: apiKeyFromEnvironment(provider, this.options.environment),
        },
        {
          signal: entry.abort.signal,
          onEvent: (event) => this.appendLine(entry, event.line),
          ...(this.options.openConversation === undefined
            ? {}
            : { openConversation: this.options.openConversation }),
        },
      );
    } catch (error: unknown) {
      outcome = {
        kind: "failed",
        reason: error instanceof Error ? error.message : String(error),
        cause: error,
      };
    }
    if (outcome.kind !== "failed") {
      entry.run = { ...entry.run, usage: outcome.usage };
      this.appendLine(entry, outcome.result);
    }
    if (entry.lifecycle.getSnapshot().matches("interrupting"))
      this.end(entry, { kind: "interrupted" });
    else if (
      entry.lifecycle.getSnapshot().matches("stopping") ||
      outcome.kind === "stopped"
    )
      this.end(entry, { kind: "stopped" });
    else if (outcome.kind === "completed")
      this.end(entry, { kind: "completed" }, outcome.result);
    else if (outcome.kind === "incomplete")
      this.end(entry, { kind: "incomplete", detail: outcome.result });
    else this.end(entry, { kind: "failed", detail: outcome.reason });
  }

  private end(
    entry: LiveRun,
    outcome: Extract<RunPhase, { state: "ended" }>["outcome"],
    response?: string,
  ): void {
    const startedAt =
      entry.run.phase.state === "ended" ? null : entry.run.phase.startedAt;
    entry.lifecycle.send({
      type: "END",
      phase: {
        state: "ended",
        startedAt,
        endedAt: this.options.now(),
        outcome,
        ...(response === undefined ? {} : { response }),
      },
    });
  }

  private persist(run: Run): void {
    const snapshot = structuredClone(run);
    this.enqueueWrite(run.id, "metadata", () => {
      const entry = this.runs.get(run.id);
      snapshot.persistenceError = entry?.outputWriteFailed
        ? entry.run.persistenceError
        : undefined;
      return this.options.store.save(snapshot);
    });
  }

  private enqueueWrite(
    id: string,
    kind: "metadata" | "output",
    write: () => Promise<void>,
  ): void {
    this.writes = this.writes
      .then(async () => {
        await write();
        const entry = this.runs.get(id);
        if (
          kind === "metadata" &&
          entry?.run.persistenceError !== undefined &&
          !entry.outputWriteFailed
        ) {
          entry.run = { ...entry.run, persistenceError: undefined };
          this.emit(entry, { type: "persistence_error", detail: null });
        }
      })
      .catch((error: unknown) => {
        const entry = this.runs.get(id);
        const detail = error instanceof Error ? error.message : String(error);
        console.error(`Could not save run ${id}:`, error);
        if (entry) {
          if (kind === "output") entry.outputWriteFailed = true;
          if (kind === "metadata" && entry.outputWriteFailed) return;
          entry.run = { ...entry.run, persistenceError: detail };
          this.emit(entry, { type: "persistence_error", detail });
        }
      });
  }
}

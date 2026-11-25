import pRetry from "p-retry";
import { match, P } from "ts-pattern";
import type { z } from "zod";
import { apiSchemas } from "./generated/api-schemas.js";
import {
  armMotionCommands,
  armPowerCommands,
} from "./generated/arm-commands.js";
import { createStationApi } from "./generated/http-routes.js";
import {
  LeaseHeldElsewhereError,
  LeaseLostError,
  StationConnectionError,
  StationHttpError,
  StationTimeoutError,
} from "./station-errors.js";
import {
  type IncomingStationMessage,
  parseRunReplay,
  parseStationEvent,
  readStationEvents,
  type StationEvent,
} from "./station-events.js";
import {
  jsonRequestInit,
  stationFetch,
  stationRequest,
} from "./station-request.js";
import {
  type ActionSequenceRecord,
  type ActionSequenceRequest,
  type ClientMessage,
  type CommandAnswer,
  type LeaseAnswer,
  type LeaseHolder,
  type LeaseStatus,
  type LiveRun,
  type MoveArmsCommand,
  type OnInterruption,
  type PreviewAnswer,
  type PreviewPose,
  type ServerMessage,
  type StationCommand,
  type StationState,
  serverMessageSchema,
  type TraceContext,
} from "./station-types.js";

const RECONNECT_CAP_MILLISECONDS = 1000;
const RECONNECT_FIRST_BACKOFF_MILLISECONDS = 100;
/** How long to wait for a connection, or for the first state. */
const CONNECT_TIMEOUT_MILLISECONDS = 10_000;
const EVENT_READ_TIMEOUT_MILLISECONDS = 30_000;
/** How long a request, or an awaited state, waits before it is given up on. */
const REPLY_TIMEOUT_MILLISECONDS = 30_000;
class StationReplayGap extends StationConnectionError {}

/** A model loop's run telemetry, in order, each record once. */
export type RunRecord = Extract<
  ServerMessage,
  { kind: "run_event" | "run_span" | "run_resync" }
>;

/** What a lease asks for besides its holder and label. */
export interface LeaseOptions {
  /** Cameras the holder cannot work without. */
  readonly requiredCameras?: readonly string[];
  /** Keep the lease and wait for approval (default), or return it, when an arm needs recovery. */
  readonly onInterruption?: OnInterruption;
}

export interface StationConnectionOptions {
  /**
   * Resume the run telemetry stream after this sequence number; 0 replays
   * everything retained, and without one the stream starts at the newest record.
   */
  readonly runAfter?: number;
}

/** How a command is sent and what is waited for. */
export interface ArmCommandOptions {
  /** Stable ID for retries, status, and cancellation. Generated if omitted. */
  readonly operationId?: string;
  /** Aborting cancels this operation and rejects. */
  readonly signal?: AbortSignal;
  /** Parents the command's span on the station. */
  readonly traceContext?: TraceContext;
  /** Wait for the command's outcome (default), or resolve once the station accepts it. */
  readonly waitForOutcome?: boolean;
}

/**
 * The lease on the arms, held by one connection. Everything that moves an arm
 * is here, so a command without the lease cannot be written. The lease token is
 * kept inside; the SDK renews the lease while it is held.
 */
export interface Lease {
  readonly status: LeaseStatus;
  readonly leaseSeconds: number;
  /** Whether this connection still holds the lease. */
  isHeld(): boolean;
  /** The commands that move one arm and its gripper. */
  arm(
    arm: string,
    options?: ArmCommandOptions,
  ): ReturnType<typeof armMotionCommands<Promise<CommandAnswer>>>;
  /** Send a command and return its admission answer. Aborting cancels its motion until it completes. */
  sendCommand(
    arm: string,
    command: StationCommand,
    expectedRevision?: number,
    traceContext?: TraceContext,
    signal?: AbortSignal,
    operationId?: string,
  ): Promise<CommandAnswer>;
  /** Cancel the exact operation, including before its admission reply. */
  cancelOperation(operationId: string): Promise<void>;
  /** Update a stream with a strictly increasing sequence within that operation. */
  updateStreamGoal(
    operationId: string,
    goal: {
      sequence: number;
      joints: number[];
      gripper_opening?: number;
      expected_revision?: number;
    },
    signal?: AbortSignal,
  ): Promise<CommandAnswer>;
  /** As `sendCommand` for several arms together. */
  sendGroup(
    command: MoveArmsCommand,
    expectedRevisions?: Record<string, number>,
    traceContext?: TraceContext,
    operationId?: string,
  ): Promise<CommandAnswer>;
  /**
   * Run a command to its outcome, and until the station's state shows where the
   * arm came to rest. Aborting `signal` cancels the submitted command.
   */
  runCommand(
    arm: string,
    command: StationCommand,
    expectedRevision?: number,
    signal?: AbortSignal,
    traceContext?: TraceContext,
    operationId?: string,
  ): Promise<CommandAnswer>;
  /** As `runCommand` for several arms together. */
  runGroup(
    command: MoveArmsCommand,
    expectedRevisions?: Record<string, number>,
    signal?: AbortSignal,
    traceContext?: TraceContext,
    operationId?: string,
  ): Promise<CommandAnswer>;
  /** Start an ordered action_sequence and return its identifier. */
  startActionSequence(
    action_sequence: ActionSequenceRequest,
    captureCameras?: readonly string[],
  ): Promise<string>;
  /** Submit a retained compilation with fresh lease and admission checks. */
  startCompiledSequence(
    compilationId: string,
    expectedRevisions?: Record<string, number>,
    continuation?: import("./generated/api-schemas.js").ApiSchemas["ProgramContinuation"],
  ): Promise<string>;
  /** Stop an arm only while this lease is current; throws `LeaseLostError` once it is not. */
  stopArm(arm: string): Promise<void>;
  waitForObservation(
    selection: import("./generated/api-schemas.js").ApiSchemas["ObservationRequest"],
    arm: string,
    timeoutMilliseconds: number,
    signal?: AbortSignal,
  ): Promise<
    import("./generated/api-schemas.js").ApiSchemas["ObservationAnswer"]
  >;
  /** Report whether a camera the lease requires gives frames; a fresh frame clears its interlock. */
  reportCamera(camera: string, available: boolean): Promise<void>;
  /** Stop the recording only while this lease is current; does nothing once it is not. */
  stopRecording(): Promise<void>;
  /** Give the lease up; the arms keep doing what they were told. */
  return(): Promise<void>;
}

export interface StationConnection {
  /** The current station HTTP base URL, including when the URL is supplied dynamically. */
  baseUrl?(): string;
  /** Connect, and keep reconnecting while the page is open. */
  start(): void;
  close(): void;
  /**
   * Resolves once connected; rejects at once when the station refuses the
   * connection, and after 10 s when it does not answer.
   */
  waitUntilConnected(): Promise<void>;
  /**
   * The commands that need no lease for one arm: `enable`, `disable`,
   * `clear_fault`, `approve_recovery` and `stop`. They run to their outcome
   * unless `waitForOutcome` is false.
   */
  arm(
    arm: string,
    options?: ArmCommandOptions,
  ): ReturnType<typeof armPowerCommands<Promise<CommandAnswer>>>;
  captureObservation(
    selection: import("./generated/api-schemas.js").ApiSchemas["ObservationRequest"],
    signal?: AbortSignal,
  ): Promise<
    import("./generated/api-schemas.js").ApiSchemas["ObservationAnswer"]
  >;
  getOperation(
    operationId: string,
  ): Promise<
    import("./generated/api-schemas.js").ApiSchemas["OperationStatus"]
  >;
  preview(arm: string, pose: PreviewPose): Promise<PreviewAnswer>;
  previewPoses(
    body: z.input<typeof apiSchemas.shape.PosePreviewBatchRequest>,
  ): Promise<
    import("./generated/api-schemas.js").ApiSchemas["PosePreviewBatchResponse"]
  >;
  compileActionSequence(
    source: ActionSequenceRequest,
  ): Promise<
    import("./generated/api-schemas.js").ApiSchemas["CompilationOutcome"]
  >;
  importCompilation(
    artifact: z.input<typeof apiSchemas.shape.CompilationArtifact>,
  ): Promise<
    import("./generated/api-schemas.js").ApiSchemas["CompilationOutcome"]
  >;
  inspectCompilation(
    compilationId: string,
  ): Promise<
    import("./generated/api-schemas.js").ApiSchemas["CompilationArtifact"]
  >;
  getActionSequence(actionSequenceId: string): Promise<ActionSequenceRecord>;
  getSequenceCameraHistory?(
    executionId: string,
    signal?: AbortSignal,
  ): ReturnType<
    ReturnType<typeof createStationApi>["getSequenceCameraHistory"]
  >;
  /** Wait for a terminal result. Aborting or timing out ends the wait; cancel the sequence to stop motion. */
  waitForActionSequence(
    actionSequenceId: string,
    options?: { signal?: AbortSignal; timeoutMilliseconds?: number },
  ): Promise<ActionSequenceRecord>;
  cancelActionSequence(actionSequenceId: string): Promise<void>;
  onActionSequence(
    listener: (action_sequence: ActionSequenceRecord | null) => void,
  ): () => void;
  /**
   * Acquire the lease when no one holds it, and return the handle that
   * commands the arms. Throws `LeaseHeldElsewhereError`, naming the holder,
   * when someone else holds it.
   */
  acquireLease(label: string, options?: LeaseOptions): Promise<Lease>;
  /** Take the lease from whoever holds it, stopping every arm first. */
  takeLease(label: string, options?: LeaseOptions): Promise<Lease>;
  /**
   * The operator's override: end whoever holds the lease. Resolves once the
   * station reports that no one holds it, so an acquire made right after succeeds.
   */
  revokeLease(): Promise<void>;
  /** The latest state, waiting up to 10 s for the first one. */
  state(): Promise<StationState>;
  /**
   * Wait for a state frame that satisfies `predicate`, checking the latest frame
   * first, and return it. Rejects with `StationTimeoutError` after 30 s.
   */
  waitForState(
    predicate: (state: StationState) => boolean,
    options?: {
      fresh?: boolean;
      signal?: AbortSignal;
      timeoutMilliseconds?: number;
    },
  ): Promise<StationState>;
  onState(listener: (state: StationState) => void): () => void;
  /** The live view of a model loop's run: a snapshot after each change. */
  onRun(listener: (run: LiveRun) => void): () => void;
  /** Run events, spans and resyncs from now on, each record once. */
  onRunRecord(listener: (record: RunRecord) => void): () => void;
  /** Report a model-loop event to the viewer's live run. */
  recordRunEvent(event: unknown): void;
  /** Report trace spans: running ones as they start, then finished. */
  recordRunSpans(
    spans: Extract<ClientMessage, { kind: "run_spans" }>["spans"],
  ): void;
  onConnectionChange(listener: (connected: boolean) => void): () => void;
  getState(): StationState | null;
  isConnected(): boolean;
  /** The lease ended without being returned: taken, revoked, returned for an interruption, lapsed or disconnected. */
  onLeaseLost(listener: () => void): () => void;
}

interface Wait {
  signal?: AbortSignal;
  /** Reject with this after the timeout. */
  timeout?: { milliseconds: number; error: () => Error };
}

function asError(error: unknown): Error {
  return error instanceof Error ? error : new Error(String(error));
}

/** Generate a UUID v4 even on LAN pages without a secure context. */
export function newOperationId(): string {
  const randomBytes = crypto.getRandomValues(new Uint8Array(16));
  randomBytes[6] = ((randomBytes[6] ?? 0) & 0x0f) | 0x40;
  randomBytes[8] = ((randomBytes[8] ?? 0) & 0x3f) | 0x80;
  const hexadecimal = Array.from(randomBytes, (byte) =>
    byte.toString(16).padStart(2, "0"),
  ).join("");
  return `${hexadecimal.slice(0, 8)}-${hexadecimal.slice(8, 12)}-${hexadecimal.slice(12, 16)}-${hexadecimal.slice(16, 20)}-${hexadecimal.slice(20)}`;
}

export function createStationConnection(
  stationUrl: string | (() => string),
  holder: LeaseHolder,
  options: StationConnectionOptions = {},
): StationConnection {
  const stateListeners = new Set<(state: StationState) => void>();
  const runListeners = new Set<(run: LiveRun) => void>();
  const runRecordListeners = new Set<(record: RunRecord) => void>();
  const actionSequenceListeners = new Set<
    (actionSequence: ActionSequenceRecord | null) => void
  >();
  let latestActionSequence: ActionSequenceRecord | null | undefined;
  const connectionListeners = new Set<(connected: boolean) => void>();
  const leaseLostListeners = new Set<() => void>();
  const changeListeners = new Set<() => void>();
  let operationLifetime = new AbortController();
  let runAfter = options.runAfter;
  let runInstance: string | undefined;
  let connectionLoop: AbortController | null = null;
  let eventCursor: string | null = null;
  let closed = false;
  let ready = false;
  let connectionGeneration = 0;
  let lastError: string | null = null;
  let leaseState:
    | { kind: "idle" }
    | { kind: "acquiring" }
    | { kind: "held"; token: string } = { kind: "idle" };
  let leaseRenewalTimer: ReturnType<typeof setTimeout> | null = null;
  const heldLeaseToken = (): string | null =>
    leaseState.kind === "held" ? leaseState.token : null;
  let latestState: StationState | null = null;
  let stateFrameCount = 0;
  let telemetryTail: Promise<void> = Promise.resolve();

  function reportTelemetry(send: () => Promise<void>): void {
    telemetryTail = telemetryTail.then(send).catch((error: unknown) => {
      console.warn("Run telemetry unavailable", error);
    });
  }

  function notify(): void {
    for (const listener of [...changeListeners]) listener();
  }

  function setReady(connected: boolean): void {
    if (ready && !connected) connectionGeneration += 1;
    ready = connected;
    for (const listener of connectionListeners) listener(connected);
    notify();
  }

  /**
   * Wait until `check` returns a value; it runs again after every change. A
   * check that throws rejects the wait, and so does closing the connection.
   */
  function waitFor<Result>(
    check: () => Result | undefined,
    wait: Wait = {},
  ): Promise<Result> {
    return new Promise((resolve, reject) => {
      let timeoutTimer: ReturnType<typeof setTimeout> | undefined;
      const finish = () => {
        changeListeners.delete(evaluate);
        wait.signal?.removeEventListener("abort", abort);
        clearTimeout(timeoutTimer);
      };
      const fail = (error: Error) => {
        finish();
        reject(error);
      };
      const abort = () => fail(asError(wait.signal?.reason));
      if (wait.signal?.aborted) {
        abort();
        return;
      }
      wait.signal?.addEventListener("abort", abort, { once: true });
      function evaluate(): void {
        if (closed) {
          fail(new StationConnectionError("The station connection is closed"));
          return;
        }
        let value: Result | undefined;
        try {
          value = check();
        } catch (error) {
          fail(asError(error));
          return;
        }
        if (value !== undefined) {
          finish();
          resolve(value);
          return;
        }
      }
      if (wait.timeout) {
        const { milliseconds, error } = wait.timeout;
        timeoutTimer = setTimeout(() => fail(error()), milliseconds);
      }
      changeListeners.add(evaluate);
      evaluate();
    });
  }

  function loseLease(): void {
    if (leaseState.kind !== "held") return;
    leaseState = { kind: "idle" };
    if (leaseRenewalTimer !== null) clearTimeout(leaseRenewalTimer);
    leaseRenewalTimer = null;
    for (const listener of leaseLostListeners) listener();
  }

  /** Deliver a run record once, in order, after a resume or a duplicate. */
  function deliverRunRecord(record: Extract<RunRecord, { seq: number }>): void {
    if (runAfter !== undefined && record.seq <= runAfter) return;
    runAfter = record.seq;
    for (const listener of runRecordListeners) listener(record);
  }

  function receive(incoming: IncomingStationMessage): void {
    if (incoming.kind === "unknown") {
      console.warn("Ignoring unknown station event", incoming.wireKind);
    }
    if (incoming.kind !== "known") return;
    match(incoming.message)
      .with({ kind: "state" }, ({ state }) => {
        latestState = state;
        stateFrameCount += 1;
        for (const listener of stateListeners) listener(state);
      })
      .with({ kind: "run" }, ({ run }) => {
        for (const listener of runListeners) listener(run);
      })
      .with({ kind: P.union("run_event", "run_span") }, deliverRunRecord)
      .with({ kind: "run_resync" }, (message) => {
        runAfter = message.newest;
        for (const listener of runRecordListeners) listener(message);
      })
      .with({ kind: "action_sequence_state" }, ({ action_sequence }) => {
        latestActionSequence = action_sequence ?? null;
        for (const listener of actionSequenceListeners)
          listener(latestActionSequence);
        notify();
      })
      .with({ kind: "lease_lost" }, loseLease)
      .with(
        {
          kind: P.union(
            "action_sequence_accepted",
            "hello",
            "frame",
            "lease",
            "command_answer",
            "preview_answer",
            "refused",
          ),
        },
        () => undefined,
      )
      .exhaustive();
  }

  async function applyStationEvent(
    event: StationEvent,
    streamUrl: URL,
    signal: AbortSignal,
    runStreamInstance: string | null,
  ): Promise<void> {
    const incoming = parseStationEvent(event);
    if (incoming.kind === "resync_required")
      throw new StationReplayGap("Station event replay requires a snapshot");
    if (incoming.kind === "ignored_event") {
      console.warn("Ignoring unknown station stream event", incoming.wireKind);
      return;
    }
    if (event.kind === "run" && incoming.kind !== "snapshot") {
      receive(incoming);
      runInstance = runStreamInstance ?? runInstance;
      return;
    }
    const previousStateFrameCount = stateFrameCount;
    if (incoming.kind === "snapshot") {
      const replay =
        runStreamInstance !== null || runAfter === undefined
          ? []
          : await replayRunRecords(streamUrl, runAfter, signal);
      signal.throwIfAborted();
      // INVARIANT: Parse both batches before replacing cached state or notifying listeners.
      latestState = null;
      runAfter ??= incoming.runSequence;
      runInstance ??= incoming.stationInstanceId;
      for (const record of replay) receive(record);
      for (const message of incoming.messages) receive(message);
    } else {
      receive(incoming);
    }
    eventCursor = event.id || eventCursor;
    lastError = null;
    if (!ready) setReady(true);
    else if (stateFrameCount !== previousStateFrameCount) notify();
  }

  async function replayRunRecords(
    streamUrl: URL,
    after: number,
    signal: AbortSignal,
  ) {
    const url = new URL("/run/records", streamUrl);
    url.searchParams.set("after", String(after));
    try {
      const payload = await stationRequest(url, { signal });
      return parseRunReplay(payload);
    } catch (error) {
      throw new StationConnectionError("Run replay failed", { cause: error });
    }
  }

  async function consumeEventStream(
    url: URL,
    attempt: AbortController,
    signal: AbortSignal,
  ): Promise<never> {
    while (true) {
      const cursor = eventCursor;
      const handshake = setTimeout(
        () =>
          attempt.abort(
            new StationTimeoutError("Station event handshake timed out"),
          ),
        CONNECT_TIMEOUT_MILLISECONDS,
      );
      const response = await fetch(url, {
        signal,
        headers: {
          ...(cursor === null ? {} : { "Last-Event-ID": cursor }),
          "X-RUA-Run-Cursor":
            runAfter === undefined
              ? "latest"
              : runInstance === undefined
                ? String(runAfter)
                : `${runInstance}:${runAfter}`,
        },
      }).finally(() => clearTimeout(handshake));
      signal.throwIfAborted();
      if (response.status === 409 && cursor !== null) {
        eventCursor = null;
        await response.body?.cancel();
        continue;
      }
      if (!response.ok)
        throw new StationConnectionError(
          `Station event stream returned ${response.status}`,
        );
      try {
        await readStationEvents(
          response,
          (event) =>
            applyStationEvent(
              event,
              url,
              signal,
              response.headers.get("X-RUA-Run-Cursor"),
            ),
          {
            milliseconds: EVENT_READ_TIMEOUT_MILLISECONDS,
            abort: () =>
              attempt.abort(
                new StationTimeoutError("Station event stream went silent"),
              ),
          },
        );
        throw new StationConnectionError("Station event stream ended");
      } catch (error) {
        signal.throwIfAborted();
        if (!(error instanceof StationReplayGap)) throw error;
        const retryWithoutCursor = eventCursor !== null && !ready;
        eventCursor = null;
        if (!retryWithoutCursor) throw error;
        await response.body?.cancel();
      }
    }
  }

  function connect(): void {
    if (connectionLoop !== null || closed) return;
    const lifetime = new AbortController();
    connectionLoop = lifetime;
    void (async () => {
      // WHY: Ending a healthy session starts a fresh retry budget. Opening HTTP
      // successfully is insufficient until we have applied a station event.
      while (!lifetime.signal.aborted) {
        await pRetry(
          async () => {
            const attempt = new AbortController();
            const signal = AbortSignal.any([lifetime.signal, attempt.signal]);
            try {
              await consumeEventStream(
                new URL("/events", baseUrl()),
                attempt,
                signal,
              );
            } catch (error) {
              lifetime.signal.throwIfAborted();
              if (!attempt.signal.aborted)
                console.warn("Station event stream ended", error);
              const wasReady = ready;
              latestState = null;
              if (!wasReady) lastError = "The station refused the connection";
              loseLease();
              setReady(false);
              if (!wasReady)
                throw new StationConnectionError(
                  "The station refused the connection",
                  { cause: error },
                );
            } finally {
              attempt.abort();
            }
          },
          {
            signal: lifetime.signal,
            retries: Number.POSITIVE_INFINITY,
            minTimeout: RECONNECT_FIRST_BACKOFF_MILLISECONDS,
            maxTimeout: RECONNECT_CAP_MILLISECONDS,
            randomize: true,
          },
        );
      }
    })()
      .catch((error: unknown) => {
        if (!lifetime.signal.aborted)
          console.warn("Station event reconnect stopped", error);
      })
      .finally(() => {
        if (connectionLoop === lifetime) connectionLoop = null;
      });
  }

  function ensureConnecting(): void {
    if (closed)
      throw new StationConnectionError("The station connection is closed");
    if (connectionLoop === null) connect();
  }

  function noConnection(): StationConnectionError {
    return new StationConnectionError(`No connection to ${baseUrl()}`);
  }

  async function waitUntilConnected(): Promise<void> {
    ensureConnecting();
    await waitFor(
      () => {
        if (ready) return true;
        if (lastError !== null) throw new StationConnectionError(lastError);
        return undefined;
      },
      {
        timeout: {
          milliseconds: CONNECT_TIMEOUT_MILLISECONDS,
          error: noConnection,
        },
      },
    );
  }

  async function commandStationInstance(
    base: string,
    needsCompletion: boolean,
    signal?: AbortSignal,
  ): Promise<string> {
    const instance = await createStationApi(
      base,
      base,
      signal,
    ).getStationInstance();
    if (needsCompletion && !instance.supports_operation_completion) {
      throw new StationConnectionError(
        "Update the station to use command completion",
      );
    }
    return instance.id;
  }

  async function sendCommand(
    arm: string,
    command: StationCommand,
    expectedRevision?: number,
    traceContext?: TraceContext,
    signal?: AbortSignal,
    awaitCompletion = false,
    operationId?: string,
  ): Promise<CommandAnswer> {
    if (signal?.aborted) throw asError(signal.reason);
    const token = heldLeaseToken();
    if (
      token === null &&
      ![
        "enable",
        "disable",
        "stop",
        "clear_fault",
        "approve_recovery",
      ].includes(command.kind)
    )
      throw new LeaseLostError();
    const base = baseUrl();
    const station_instance_id = await commandStationInstance(
      base,
      awaitCompletion || signal !== undefined,
      signal,
    );
    if (signal?.aborted) throw asError(signal.reason);
    const operation_id = operationId ?? newOperationId();
    const submission = apiSchemas.shape.CommandSubmissionRequest.parse({
      arm,
      command,
      station_instance_id,
      operation_id,
      lease_token: token,
      ...(expectedRevision === undefined
        ? {}
        : { expected_revision: expectedRevision }),
      ...(traceContext ? { trace_context: traceContext } : {}),
    });
    return submitHttpCommand(
      base,
      (api) => api.submitCommandHttp(submission),
      { operation_id, station_instance_id, token },
      signal,
      awaitCompletion,
    );
  }

  async function sendGroup(
    command: MoveArmsCommand,
    expectedRevisions?: Record<string, number>,
    traceContext?: TraceContext,
    signal?: AbortSignal,
    awaitCompletion = false,
    operationId?: string,
  ): Promise<CommandAnswer> {
    if (signal?.aborted) throw asError(signal.reason);
    const token = heldLeaseToken();
    if (token === null) throw new LeaseLostError();
    const base = baseUrl();
    const station_instance_id = await commandStationInstance(
      base,
      awaitCompletion || signal !== undefined,
      signal,
    );
    if (signal?.aborted) throw asError(signal.reason);
    const operation_id = operationId ?? newOperationId();
    const submission = apiSchemas.shape.GroupCommandSubmissionRequest.parse({
      command,
      operation_id,
      station_instance_id,
      lease_token: token,
      ...(expectedRevisions ? { expected_revisions: expectedRevisions } : {}),
      ...(traceContext ? { trace_context: traceContext } : {}),
    });
    return submitHttpCommand(
      base,
      (api) => api.submitGroupCommandHttp(submission),
      {
        operation_id,
        station_instance_id,
        token,
      },
      signal,
      awaitCompletion,
    );
  }

  function parseCommandAnswer(
    operationId: string,
    answer: unknown,
  ): CommandAnswer {
    const message = serverMessageSchema.parse({
      kind: "command_answer",
      request_id: operationId,
      answer,
    });
    if (message.kind !== "command_answer")
      throw new StationConnectionError("Unexpected command response");
    return message.answer;
  }

  async function retryOperationRequest<Result>(
    request: (signal: AbortSignal) => Promise<Result>,
    signal: AbortSignal,
  ): Promise<Result> {
    const deadline = performance.now() + REPLY_TIMEOUT_MILLISECONDS;
    while (true) {
      signal.throwIfAborted();
      const remaining = Math.max(0, Math.ceil(deadline - performance.now()));
      try {
        return await request(
          AbortSignal.any([signal, AbortSignal.timeout(remaining)]),
        );
      } catch (error) {
        signal.throwIfAborted();
        if (
          !(error instanceof TypeError) ||
          performance.now() + 100 >= deadline
        )
          throw error;
        await new Promise((resolve) => setTimeout(resolve, 100));
      }
    }
  }

  async function submitHttpCommand(
    base: string,
    submit: (api: ReturnType<typeof createStationApi>) => Promise<unknown>,
    identity: {
      operation_id: string;
      station_instance_id: string;
      token: string | null;
    },
    signal?: AbortSignal,
    awaitCompletion = false,
  ): Promise<CommandAnswer> {
    const { operation_id, station_instance_id, token } = identity;
    const cancel = () => {
      if (token === null) return;
      void createStationApi(base, base)
        .cancelOperationHttp(operation_id, {
          station_instance_id,
          lease_token: token,
        })
        .catch((error: unknown) =>
          console.warn("Could not cancel command", error),
        );
    };
    const requestSignal = signal
      ? AbortSignal.any([signal, operationLifetime.signal])
      : operationLifetime.signal;
    requestSignal.throwIfAborted();
    signal?.addEventListener("abort", cancel, { once: true });
    let retainedCancellation = false;
    const completion = async () => {
      while (true) {
        const result = await retryOperationRequest(
          (attempt) =>
            createStationApi(base, base, attempt).waitOperationHttp(
              operation_id,
              { id: station_instance_id },
            ),
          requestSignal,
        );
        if (result !== null) return result;
      }
    };
    try {
      const answer = parseCommandAnswer(
        operation_id,
        await retryOperationRequest(
          (attempt) => submit(createStationApi(base, base, attempt)),
          requestSignal,
        ),
      );
      if (answer.status === "refused") return answer;
      if (!awaitCompletion) {
        if (
          signal &&
          answer.records.some(({ record }) => record.phase.kind === "pending")
        ) {
          retainedCancellation = true;
          // INVARIANT: Failure to read completion leaves the command cancellable.
          void pRetry(
            () =>
              completion().catch((cause: unknown) => {
                throw new StationConnectionError(
                  "Could not confirm command completion",
                  { cause },
                );
              }),
            {
              signal: requestSignal,
              retries: Number.POSITIVE_INFINITY,
              maxTimeout: RECONNECT_CAP_MILLISECONDS,
              unref: true,
            },
          )
            .catch(() => {})
            .finally(() => signal.removeEventListener("abort", cancel));
        }
        return answer;
      }

      const { records, state_sequence } = await completion();
      if (ready) {
        const prefix = `${station_instance_id}:`;
        const requiredSequence = BigInt(state_sequence);
        await waitFor(
          () => {
            if (!ready) return true;
            return latestState !== null &&
              eventCursor?.startsWith(prefix) &&
              BigInt(eventCursor.slice(prefix.length)) >= requiredSequence
              ? true
              : undefined;
          },
          {
            signal: requestSignal,
            timeout: {
              milliseconds: REPLY_TIMEOUT_MILLISECONDS,
              error: () =>
                new StationTimeoutError(
                  "Timed out waiting for the station state after command completion",
                ),
            },
          },
        );
      }
      return parseCommandAnswer(operation_id, { status: "accepted", records });
    } catch (error) {
      if (!requestSignal.aborted && !(error instanceof StationHttpError))
        cancel();
      throw error;
    } finally {
      if (!retainedCancellation) signal?.removeEventListener("abort", cancel);
    }
  }

  async function getActionSequence(
    actionSequenceId: string,
  ): Promise<ActionSequenceRecord> {
    return stationApi().getActionSequence(actionSequenceId);
  }

  async function state(): Promise<StationState> {
    ensureConnecting();
    return waitFor(() => latestState ?? undefined, {
      timeout: {
        milliseconds: CONNECT_TIMEOUT_MILLISECONDS,
        error: () =>
          new StationTimeoutError("Timed out waiting for the station state"),
      },
    });
  }

  async function waitForState(
    predicate: (state: StationState) => boolean,
    options: NonNullable<Parameters<StationConnection["waitForState"]>[1]> = {},
    afterFrameCount = options.fresh ? stateFrameCount : undefined,
  ): Promise<StationState> {
    ensureConnecting();
    return waitFor(
      () =>
        latestState !== null &&
        (afterFrameCount === undefined || stateFrameCount > afterFrameCount) &&
        predicate(latestState)
          ? latestState
          : undefined,
      {
        ...(options.signal === undefined ? {} : { signal: options.signal }),
        timeout: {
          milliseconds:
            options.timeoutMilliseconds ?? REPLY_TIMEOUT_MILLISECONDS,
          error: () =>
            new StationTimeoutError(
              "Timed out waiting for the awaited station state",
            ),
        },
      },
    );
  }

  async function returnLease(keepalive = false): Promise<void> {
    const token = heldLeaseToken();
    if (token === null) return;
    if (keepalive) loseLease();
    else {
      leaseState = { kind: "idle" };
      if (leaseRenewalTimer !== null) clearTimeout(leaseRenewalTimer);
      leaseRenewalTimer = null;
    }
    try {
      if (keepalive) {
        await stationFetch(new URL("/leases/release", baseUrl()), {
          ...jsonRequestInit("POST", { lease_token: token }),
          keepalive: true,
        });
      } else {
        await stationApi().releaseLeaseHttp({ lease_token: token });
      }
    } catch (error) {
      if (!(error instanceof StationHttpError && error.status === 409))
        throw error;
    }
  }

  async function requestLease(
    kind: "acquire_lease" | "take_lease",
    label: string,
    options: LeaseOptions,
  ): Promise<LeaseAnswer> {
    if (leaseState.kind !== "idle")
      throw new StationConnectionError(
        "This client already holds or is acquiring a lease",
      );
    leaseState = { kind: "acquiring" };
    try {
      await waitUntilConnected();
      const acquisitionGeneration = connectionGeneration;
      const instance = await stationApi().getStationInstance();
      const body = apiSchemas.shape.LeaseAcquireRequest.parse({
        request_id: newOperationId(),
        station_instance_id: instance.id,
        take: kind === "take_lease",
        request: {
          holder,
          label,
          required_cameras: options.requiredCameras ?? [],
          on_interruption: options.onInterruption ?? "wait_for_approval",
        },
      });
      let grant: z.infer<typeof apiSchemas.shape.LeaseGrant>;
      const deadline = Date.now() + REPLY_TIMEOUT_MILLISECONDS;
      while (true) {
        try {
          grant = await stationApi().acquireLeaseHttp(body);
          break;
        } catch (error) {
          if (
            error instanceof StationHttpError &&
            error.status === 409 &&
            error.body.startsWith("The lease is held by ")
          ) {
            // CONSTRAINT: A holder can release between the refusal and the state read.
            const status = await stationApi()
              .getState()
              .then(
                (state) => state.lease ?? null,
                () => null,
              );
            return { kind: "holder", status };
          }
          if (!(error instanceof TypeError) || Date.now() >= deadline)
            throw error;
          await new Promise((resolve) => setTimeout(resolve, 100));
        }
      }
      try {
        await stationApi().renewLeaseHttp({ lease_token: grant.lease_token });
      } catch (error: unknown) {
        try {
          await stationApi().releaseLeaseHttp({
            lease_token: grant.lease_token,
          });
        } catch (releaseError: unknown) {
          console.warn(
            "Could not release lease after initial renewal failed",
            releaseError,
          );
        }
        throw error;
      }
      if (closed || !ready || connectionGeneration !== acquisitionGeneration) {
        await stationApi().releaseLeaseHttp({ lease_token: grant.lease_token });
        throw new StationConnectionError(
          "The station connection was lost while acquiring the lease",
        );
      }
      leaseState = { kind: "held", token: grant.lease_token };
      scheduleLeaseRenewal(grant.lease_token, grant.lease_seconds);
      return { kind: "granted", ...grant };
    } finally {
      if (leaseState.kind === "acquiring") leaseState = { kind: "idle" };
    }
  }

  function scheduleLeaseRenewal(token: string, leaseSeconds: number): void {
    leaseRenewalTimer = setTimeout(
      async () => {
        leaseRenewalTimer = null;
        if (heldLeaseToken() !== token) return;
        if (!ready || latestState === null) {
          loseLease();
          return;
        }
        try {
          await stationApi().renewLeaseHttp({ lease_token: token });
          if (heldLeaseToken() === token)
            scheduleLeaseRenewal(token, leaseSeconds);
        } catch (error) {
          console.warn("Lease renewal failed", error);
          if (heldLeaseToken() === token) loseLease();
        }
      },
      Math.max(100, (leaseSeconds * 1000) / 3),
    );
  }

  function baseUrl(): string {
    return typeof stationUrl === "string" ? stationUrl : stationUrl();
  }

  function stationApi(
    signal?: AbortSignal,
  ): ReturnType<typeof createStationApi> {
    const base = baseUrl();
    return createStationApi(base, base, signal);
  }

  function sendAs(
    arm: string,
    options: ArmCommandOptions,
  ): (
    command: StationCommand,
    expectedRevision?: number,
  ) => Promise<CommandAnswer> {
    return (command, expectedRevision) =>
      runCommand(
        arm,
        command,
        expectedRevision,
        options.signal,
        options.traceContext,
        options.waitForOutcome,
        options.operationId,
      );
  }

  function runCommand(
    arm: string,
    command: StationCommand,
    expectedRevision?: number,
    signal?: AbortSignal,
    traceContext?: TraceContext,
    waitForOutcome = true,
    operationId?: string,
  ): Promise<CommandAnswer> {
    return sendCommand(
      arm,
      command,
      expectedRevision,
      traceContext,
      signal,
      waitForOutcome,
      operationId,
    );
  }

  function runGroup(
    command: MoveArmsCommand,
    expectedRevisions?: Record<string, number>,
    signal?: AbortSignal,
    traceContext?: TraceContext,
    operationId?: string,
  ): Promise<CommandAnswer> {
    return sendGroup(
      command,
      expectedRevisions,
      traceContext,
      signal,
      true,
      operationId,
    );
  }

  async function startActionSequence(
    action_sequence: ActionSequenceRequest,
    captureCameras?: readonly string[],
  ): Promise<string> {
    const token = heldLeaseToken();
    if (token === null) throw new LeaseLostError();
    const submission = apiSchemas.shape.ActionSequenceStartRequest.parse({
      request_id: newOperationId(),
      station_instance_id: (await stationApi().getStationInstance()).id,
      lease_token: token,
      action_sequence,
      ...(captureCameras?.length ? { capture_cameras: captureCameras } : {}),
    });
    return retryOperationRequest(
      (signal) => stationApi(signal).startActionSequenceHttp(submission),
      operationLifetime.signal,
    );
  }

  async function grantedLease(
    kind: "acquire_lease" | "take_lease",
    label: string,
    options: LeaseOptions,
  ): Promise<Lease> {
    const answer = await requestLease(kind, label, options);
    if (answer.kind !== "granted") throw new LeaseHeldElsewhereError(answer);
    const token = answer.lease_token;
    const isHeld = () => heldLeaseToken() === token;
    const requireCurrentLease = () => {
      if (!isHeld()) throw new LeaseLostError();
    };
    return {
      status: answer.status,
      leaseSeconds: answer.lease_seconds,
      isHeld,
      arm: (arm, armOptions = {}) =>
        armMotionCommands((command, revision) => {
          requireCurrentLease();
          return sendAs(arm, armOptions)(command, revision);
        }),
      sendCommand(arm, command, revision, traceContext, signal, operationId) {
        requireCurrentLease();
        return sendCommand(
          arm,
          command,
          revision,
          traceContext,
          signal,
          false,
          operationId,
        );
      },
      async cancelOperation(operationId) {
        requireCurrentLease();
        const station_instance_id = (await stationApi().getStationInstance())
          .id;
        requireCurrentLease();
        await stationApi().cancelOperationHttp(operationId, {
          station_instance_id,
          lease_token: token,
        });
      },
      async updateStreamGoal(operationId, goal, signal) {
        requireCurrentLease();
        const station_instance_id = (await stationApi().getStationInstance())
          .id;
        requireCurrentLease();
        const answer = await stationApi(signal).updateStreamGoalHttp(
          operationId,
          {
            ...goal,
            station_instance_id,
            lease_token: token,
            expected_revision: goal.expected_revision ?? null,
            gripper_opening: goal.gripper_opening ?? null,
          },
        );
        return parseCommandAnswer(operationId, answer);
      },
      sendGroup(command, revisions, traceContext, operationId) {
        requireCurrentLease();
        return sendGroup(
          command,
          revisions,
          traceContext,
          undefined,
          false,
          operationId,
        );
      },
      runCommand(arm, command, revision, signal, traceContext, operationId) {
        requireCurrentLease();
        return runCommand(
          arm,
          command,
          revision,
          signal,
          traceContext,
          true,
          operationId,
        );
      },
      runGroup(...args) {
        requireCurrentLease();
        return runGroup(...args);
      },
      async startCompiledSequence(
        compilationId,
        expectedRevisions = {},
        continuation,
      ) {
        if (!isHeld()) throw new LeaseLostError();
        const submission = {
          request_id: newOperationId(),
          station_instance_id: (await stationApi().getStationInstance()).id,
          compilation_id: compilationId,
          continuation: continuation ?? null,
          lease_token: token,
          expected_revisions: expectedRevisions,
        };
        return retryOperationRequest(
          (signal) => stationApi(signal).runCompiledSequenceHttp(submission),
          operationLifetime.signal,
        );
      },
      startActionSequence(actionSequence, captureCameras) {
        requireCurrentLease();
        return startActionSequence(actionSequence, captureCameras);
      },
      async stopArm(arm) {
        if (!isHeld()) throw new LeaseLostError();
        try {
          await stationApi().stopLeasedArmHttp(arm, { lease_token: token });
        } catch (error) {
          if (error instanceof StationHttpError && error.status === 403)
            throw new LeaseLostError();
          throw error;
        }
      },
      async waitForObservation(selection, arm, timeoutMilliseconds, signal) {
        requireCurrentLease();
        const requestSignal = signal
          ? AbortSignal.any([signal, operationLifetime.signal])
          : operationLifetime.signal;
        try {
          return await stationApi(requestSignal).captureObservationHttp({
            ...selection,
            wait: {
              arm,
              lease_token: token,
              timeout_milliseconds: timeoutMilliseconds,
            },
          });
        } catch (error) {
          if (
            !isHeld() ||
            (error instanceof StationHttpError && error.status === 403)
          )
            throw new LeaseLostError();
          throw error;
        }
      },
      async reportCamera(camera, available) {
        if (!isHeld()) throw new LeaseLostError();
        try {
          await stationApi().reportLeaseCameraHttp({
            lease_token: token,
            camera,
            available,
          });
        } catch (error) {
          if (error instanceof StationHttpError && error.status === 403)
            throw new LeaseLostError();
          throw error;
        }
      },
      async stopRecording() {
        try {
          await stationApi().stopRecordingForLease({ lease_token: token });
        } catch (error) {
          if (!(error instanceof StationHttpError && error.status === 409))
            throw error;
        }
      },
      async return() {
        if (isHeld()) await returnLease();
      },
    };
  }

  const previewPoses: StationConnection["previewPoses"] = (body) =>
    stationApi().previewPoses(
      apiSchemas.shape.PosePreviewBatchRequest.parse(body),
    );

  return {
    baseUrl,
    start() {
      if (closed) operationLifetime = new AbortController();
      closed = false;
      if (connectionLoop === null) connect();
    },
    close() {
      closed = true;
      operationLifetime.abort(
        new StationConnectionError("The station connection is closed"),
      );
      void returnLease(true).catch((error: unknown) =>
        console.warn("Could not release lease", error),
      );
      const previous = connectionLoop;
      connectionLoop = null;
      latestState = null;
      loseLease();
      setReady(false);
      previous?.abort();
    },
    waitUntilConnected,
    arm: (arm, options = {}) => armPowerCommands(sendAs(arm, options)),
    captureObservation: (selection, signal) =>
      stationApi(signal).captureObservationHttp(selection),
    getOperation: (operationId) => stationApi().getOperation(operationId),
    getActionSequence,
    getSequenceCameraHistory: (id, signal) =>
      stationApi(signal).getSequenceCameraHistory(id),
    async waitForActionSequence(actionSequenceId, options = {}) {
      const { signal, timeoutMilliseconds } = options;
      const timeout =
        timeoutMilliseconds === undefined
          ? undefined
          : AbortSignal.timeout(Math.max(0, Math.trunc(timeoutMilliseconds)));
      const requestSignal = AbortSignal.any([
        operationLifetime.signal,
        ...(signal ? [signal] : []),
        ...(timeout ? [timeout] : []),
      ]);
      try {
        while (true) {
          const record = await retryOperationRequest(
            (attempt) =>
              stationApi(attempt).waitActionSequenceCompletion(
                actionSequenceId,
              ),
            requestSignal,
          );
          if (record !== null)
            return apiSchemas.shape.ActionSequenceRecord.parse(record);
        }
      } catch (error) {
        if (timeout?.aborted && requestSignal.reason === timeout.reason) {
          throw new StationTimeoutError(
            `Timed out waiting for action sequence ${actionSequenceId}`,
          );
        }
        throw error;
      }
    },
    async cancelActionSequence(actionSequenceId) {
      try {
        await stationApi().cancelActionSequenceHttp(actionSequenceId);
      } catch (error) {
        if (!(error instanceof StationHttpError && error.status === 404))
          throw error;
      }
    },
    onActionSequence(listener) {
      actionSequenceListeners.add(listener);
      if (latestActionSequence !== undefined) listener(latestActionSequence);
      return () => {
        actionSequenceListeners.delete(listener);
      };
    },
    async preview(arm, pose) {
      const response = await previewPoses({
        arm,
        poses: [
          {
            id: newOperationId(),
            pose: { ...pose, orientation: pose.orientation ?? null },
            label: null,
          },
        ],
      });
      const result = response.results[0];
      if (!result)
        throw new StationConnectionError("Preview result is missing");
      const message = serverMessageSchema.parse({
        kind: "preview_answer",
        request_id: null,
        answer: result.answer,
      });
      if (message.kind !== "preview_answer")
        throw new StationConnectionError("Unexpected preview result");
      return message.answer;
    },
    compileActionSequence(source) {
      return stationApi().compileActionSequence(
        apiSchemas.shape.ActionSequenceRequest.parse(source),
      );
    },
    importCompilation(artifact) {
      return stationApi().importCompilation(
        apiSchemas.shape.CompilationArtifact.parse(artifact),
      );
    },
    inspectCompilation(compilationId) {
      return stationApi().inspectCompilation(compilationId);
    },
    previewPoses,
    acquireLease: (label, leaseOptions = {}) =>
      grantedLease("acquire_lease", label, leaseOptions),
    takeLease: (label, leaseOptions = {}) =>
      grantedLease("take_lease", label, leaseOptions),
    async revokeLease() {
      const answer = await requestLease("take_lease", "operator override", {});
      if (answer.kind !== "granted") throw new LeaseHeldElsewhereError(answer);
      // CONSTRAINT: The cached frame can predate the take and show no holder, so only a
      // frame received after the take's reply counts.
      const framesBeforeReturn = stateFrameCount;
      await returnLease();
      await waitForState(
        (current) => current.lease === null,
        {},
        framesBeforeReturn,
      );
    },
    state,
    waitForState,
    getState: () => latestState,
    isConnected: () => ready,
    onState(listener) {
      stateListeners.add(listener);
      return () => {
        stateListeners.delete(listener);
      };
    },
    onRun(listener) {
      runListeners.add(listener);
      return () => {
        runListeners.delete(listener);
      };
    },
    onRunRecord(listener) {
      runRecordListeners.add(listener);
      return () => {
        runRecordListeners.delete(listener);
      };
    },
    recordRunEvent(event) {
      reportTelemetry(() => stationApi().recordRunEventHttp(event));
    },
    recordRunSpans(spans) {
      reportTelemetry(() =>
        stationApi().recordRunSpansHttp(
          apiSchemas.shape.RunSpansRequest.parse({ spans }),
        ),
      );
    },
    onConnectionChange(listener) {
      connectionListeners.add(listener);
      return () => {
        connectionListeners.delete(listener);
      };
    },
    onLeaseLost(listener) {
      leaseLostListeners.add(listener);
      return () => {
        leaseLostListeners.delete(listener);
      };
    },
  };
}

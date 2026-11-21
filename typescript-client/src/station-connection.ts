// Station requests use HTTP; observations use a resumable event stream.

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
import { readStationEvents, type StationEvent } from "./station-events.js";
import { jsonRequestInit, stationFetch } from "./station-request.js";
import {
  type ActionSequenceRecord,
  type ActionSequenceRequest,
  type ClientMessage,
  type CommandAnswer,
  type CommandRecord,
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

/** The first retry is immediate, then 100 ms doubling up to the cap. */
const RECONNECT_CAP_MILLISECONDS = 1000;
const RECONNECT_FIRST_BACKOFF_MILLISECONDS = 100;
/** How long to wait for a connection, or for the first state. */
const CONNECT_TIMEOUT_MILLISECONDS = 10_000;
const EVENT_READ_TIMEOUT_MILLISECONDS = 30_000;
/** How long a request, or an awaited state, waits before it is given up on. */
const REPLY_TIMEOUT_MILLISECONDS = 30_000;
/** How long a command's outcome is awaited across a lost connection. */
const DISCONNECT_PATIENCE_MILLISECONDS = 30_000;
const RETAINED_RECORDS = 10_000;

type ArmRecord = Extract<
  CommandAnswer,
  { status: "accepted" }
>["records"][number];

type Frame = Extract<ServerMessage, { kind: "frame" }>["frame"];

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
  /** Aborting stops the arm and rejects. */
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
  ): ReturnType<typeof armMotionCommands<Promise<CommandAnswer>>> & {
    /** Return to the enable-time pose and close the gripper at up to 2 rad/s; always waits for completion. */
    rehome(expectedRevision?: number): Promise<CommandAnswer>;
  };
  /** Send a command and return its admission answer. Aborting cancels its motion until it completes. */
  sendCommand(
    arm: string,
    command: StationCommand,
    expectedRevision?: number,
    traceContext?: TraceContext,
    signal?: AbortSignal,
  ): Promise<CommandAnswer>;
  /** As `sendCommand` for several arms together. */
  sendGroup(
    command: MoveArmsCommand,
    expectedRevisions?: Record<string, number>,
    traceContext?: TraceContext,
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
  ): Promise<CommandAnswer>;
  /** As `runCommand` for several arms together. */
  runGroup(
    command: MoveArmsCommand,
    expectedRevisions?: Record<string, number>,
    signal?: AbortSignal,
    traceContext?: TraceContext,
  ): Promise<CommandAnswer>;
  /** Start an ordered action_sequence and return its identifier. */
  startActionSequence(action_sequence: ActionSequenceRequest): Promise<string>;
  /** Submit a retained compilation with fresh lease and admission checks. */
  startCompiledSequence(
    compilationId: string,
    expectedRevisions?: Record<string, number>,
    continuation?: import("./generated/api-schemas.js").ApiSchemas["ProgramContinuation"],
  ): Promise<string>;
  /** Stop an arm only while this lease is current; throws `LeaseLostError` once it is not. */
  stopArm(arm: string): Promise<void>;
  /** Report whether a camera the lease requires gives frames; a fresh frame clears its interlock. */
  reportCamera(camera: string, available: boolean): Promise<void>;
  /** Stop the recording only while this lease is current; does nothing once it is not. */
  stopRecording(): Promise<void>;
  /** Give the lease up; the arms keep doing what they were told. */
  return(): Promise<void>;
}

export interface StationConnection {
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
  /**
   * Wait for a command's record to finish, across reconnects, then for the first
   * state frame that reflects it: its arm no longer has it active and reports
   * either this command or a later completion as its last command. A lost
   * connection ends the second wait.
   */
  waitForOutcome(commandId: string): Promise<CommandRecord>;
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
  getActionSequence(action_sequenceId: string): Promise<ActionSequenceRecord>;
  waitForActionSequence(
    action_sequenceId: string,
  ): Promise<ActionSequenceRecord>;
  cancelActionSequence(action_sequenceId: string): Promise<void>;
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
  /** Reject once the connection has been down for the patience period. */
  patient?: boolean;
}

function asError(error: unknown): Error {
  return error instanceof Error ? error : new Error(String(error));
}

/** Generate a UUID v4 even on LAN pages without a secure context. */
export function newRequestId(): string {
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
  const action_sequenceListeners = new Set<
    (action_sequence: ActionSequenceRecord | null) => void
  >();
  let latestActionSequence: ActionSequenceRecord | null | undefined;
  const connectionListeners = new Set<(connected: boolean) => void>();
  const leaseLostListeners = new Set<() => void>();
  const changeListeners = new Set<() => void>();
  const records = new Map<string, ArmRecord>();
  const resumeAfter = new Map<string, number>();
  let runAfter = options.runAfter;
  let eventStream: AbortController | null = null;
  let eventCursor: string | null = null;
  let closed = false;
  let reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  let ready = false;
  let connectionGeneration = 0;
  let failuresInARow = 0;
  let attemptStartedAt = 0;
  let disconnectedSince: number | null = null;
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

  /** The wait after an attempt that failed having run for `attemptMilliseconds`. */
  function reconnectDelay(attemptMilliseconds: number): number {
    // An attempt that lasted as long as the cap starts the backoff over.
    if (attemptMilliseconds >= RECONNECT_CAP_MILLISECONDS) failuresInARow = 0;
    const failuresBefore = failuresInARow;
    failuresInARow += 1;
    return failuresBefore === 0
      ? 0
      : Math.min(
          RECONNECT_CAP_MILLISECONDS,
          RECONNECT_FIRST_BACKOFF_MILLISECONDS * 2 ** (failuresBefore - 1),
        );
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
      let patienceTimer: ReturnType<typeof setTimeout> | undefined;
      const finish = () => {
        changeListeners.delete(evaluate);
        wait.signal?.removeEventListener("abort", abort);
        clearTimeout(timeoutTimer);
        clearTimeout(patienceTimer);
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
      const unreachable = () =>
        new StationConnectionError(
          `${typeof stationUrl === "string" ? stationUrl : stationUrl()} has been unreachable for too long`,
        );
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
        if (!wait.patient) return;
        if (disconnectedSince === null) {
          clearTimeout(patienceTimer);
          patienceTimer = undefined;
          return;
        }
        const remaining =
          disconnectedSince + DISCONNECT_PATIENCE_MILLISECONDS - Date.now();
        if (remaining <= 0) {
          fail(unreachable());
        } else if (patienceTimer === undefined) {
          patienceTimer = setTimeout(() => {
            patienceTimer = undefined;
            evaluate();
          }, remaining);
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

  function rememberRecord(armRecord: ArmRecord): void {
    const { record } = armRecord;
    const previous = records.get(record.command_id);
    // A late, unfinished revision never replaces the finished one.
    if (
      previous !== undefined &&
      previous.record.completion !== null &&
      record.completion === null
    )
      return;
    records.delete(record.command_id);
    records.set(record.command_id, armRecord);
    if (records.size > RETAINED_RECORDS) {
      const oldest = records.keys().next().value;
      if (oldest !== undefined) records.delete(oldest);
    }
    notify();
  }

  /** Keep a frame's records and advance the arm's resume cursor. */
  function keepFrame(arm: string, frame: Frame): void {
    const previous = resumeAfter.get(arm);
    if (
      typeof frame.seq === "number" &&
      previous !== undefined &&
      frame.seq <= previous
    )
      return;
    const cursor = match(frame.event)
      .with({ kind: "record" }, ({ kind, ...record }) => {
        void kind;
        rememberRecord({ arm, record });
        return frame.seq;
      })
      .with({ kind: "snapshot" }, (snapshot) => {
        for (const retained of snapshot.log) keepFrame(arm, retained);
        for (const record of snapshot.records) rememberRecord({ arm, record });
        return snapshot.seq;
      })
      .with(
        {
          kind: P.union(
            "state",
            "reply",
            "resync",
            "mode",
            "rejected",
            "tripped",
            "controller_unavailable",
            "controller_available",
          ),
        },
        () => frame.seq,
      )
      .exhaustive();
    if (typeof cursor === "number") resumeAfter.set(arm, cursor);
  }

  /** Deliver a run record once, in order, after a resume or a duplicate. */
  function deliverRunRecord(record: Extract<RunRecord, { seq: number }>): void {
    if (runAfter !== undefined && record.seq <= runAfter) return;
    runAfter = record.seq;
    for (const listener of runRecordListeners) listener(record);
  }

  async function finishedAnswer(
    answer: CommandAnswer,
    signal?: AbortSignal,
  ): Promise<CommandAnswer> {
    if (answer.status === "refused") return answer;
    const completed = await Promise.all(
      answer.records.map(async (entry) => ({
        arm: entry.arm,
        record: await waitForOutcome(entry.record.command_id, signal),
      })),
    );
    return { ...answer, records: completed };
  }

  async function waitForOutcome(
    commandId: string,
    signal?: AbortSignal,
  ): Promise<CommandRecord> {
    const { arm: commandArm, record } = await waitFor(
      () => {
        const latest = records.get(commandId);
        return latest?.record.completion ? latest : undefined;
      },
      { patient: true, signal },
    );
    await waitFor(
      () => {
        if (!ready) return true;
        if (latestState === null) return undefined;
        const arm = latestState.arms.find((arm) => arm.arm === commandArm);
        if (arm === undefined || arm.active_command?.command_id === commandId)
          return undefined;
        const finishedAt = record.completion?.finished_at ?? 0;
        // Commands that finish in one cycle show only the latest on that arm.
        return arm.last_command?.command_id === commandId ||
          (arm.last_command?.completion?.finished_at ?? -1) >= finishedAt
          ? true
          : undefined;
      },
      { signal },
    );
    return record;
  }

  function receive(text: string): void {
    const raw: unknown = JSON.parse(text);
    // Arm samples are most of the traffic; only the cursor, records and
    // snapshots of a frame are read.
    if (
      typeof raw === "object" &&
      raw !== null &&
      "kind" in raw &&
      raw.kind === "frame" &&
      "arm" in raw &&
      typeof raw.arm === "string" &&
      "frame" in raw &&
      typeof raw.frame === "object" &&
      raw.frame !== null &&
      "event" in raw.frame &&
      typeof raw.frame.event === "object" &&
      raw.frame.event !== null &&
      "kind" in raw.frame.event &&
      raw.frame.event.kind !== "record" &&
      raw.frame.event.kind !== "snapshot"
    ) {
      if ("seq" in raw.frame && typeof raw.frame.seq === "number") {
        const previous = resumeAfter.get(raw.arm);
        if (previous === undefined || raw.frame.seq > previous)
          resumeAfter.set(raw.arm, raw.frame.seq);
      }
      return;
    }
    match(serverMessageSchema.parse(raw))
      .with({ kind: "state" }, ({ state }) => {
        latestState = state;
        stateFrameCount += 1;
        for (const listener of stateListeners) listener(state);
        notify();
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
        for (const listener of action_sequenceListeners)
          listener(latestActionSequence);
        notify();
      })
      .with({ kind: "lease_lost" }, loseLease)
      .with({ kind: "frame" }, ({ arm, frame }) => keepFrame(arm, frame))
      // The event stream applies updates. Request replies are handled by their callers.
      .with(
        {
          kind: P.union(
            "action_sequence_accepted",
            "hello",
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
    signal: AbortSignal,
  ): Promise<void> {
    if (event.kind === "resync_required") {
      eventCursor = null;
      throw new StationConnectionError(
        "Station event replay requires a snapshot",
      );
    }
    const payload: unknown = JSON.parse(event.data);
    if (
      typeof payload === "object" &&
      payload !== null &&
      "kind" in payload &&
      payload.kind === "resync_required"
    ) {
      eventCursor = null;
      throw new StationConnectionError(
        "Station event replay requires a snapshot",
      );
    }
    if (event.kind === "snapshot") {
      if (
        typeof payload !== "object" ||
        payload === null ||
        !("items" in payload) ||
        !Array.isArray(payload.items) ||
        !("run_sequence" in payload) ||
        typeof payload.run_sequence !== "number"
      )
        throw new StationConnectionError("Invalid station snapshot");
      latestState = null;
      records.clear();
      resumeAfter.clear();
      if (runAfter === undefined) {
        runAfter = payload.run_sequence;
      } else {
        const base = typeof stationUrl === "string" ? stationUrl : stationUrl();
        const url = new URL("/run/records", base);
        url.searchParams.set("after", String(runAfter));
        const response = await fetch(url, { signal });
        if (!response.ok) throw new StationConnectionError("Run replay failed");
        const replay: unknown = await response.json();
        if (!Array.isArray(replay))
          throw new StationConnectionError("Invalid run replay");
        for (const record of replay) receive(JSON.stringify(record));
      }
      for (const item of payload.items) receive(JSON.stringify(item));
    } else if (event.kind === "station") {
      receive(event.data);
    } else {
      return;
    }
    eventCursor = event.id || eventCursor;
    disconnectedSince = null;
    lastError = null;
    if (!ready) setReady(true);
  }

  function connect(): void {
    if (eventStream !== null || closed) return;
    if (reconnectTimer !== null) {
      clearTimeout(reconnectTimer);
      reconnectTimer = null;
    }
    attemptStartedAt = Date.now();
    const connecting = new AbortController();
    eventStream = connecting;
    void (async () => {
      const base = typeof stationUrl === "string" ? stationUrl : stationUrl();
      const handshake = setTimeout(
        () =>
          connecting.abort(
            new StationTimeoutError("Station event handshake timed out"),
          ),
        CONNECT_TIMEOUT_MILLISECONDS,
      );
      const response = await fetch(new URL("/events", base), {
        signal: connecting.signal,
        headers: eventCursor === null ? {} : { "Last-Event-ID": eventCursor },
      }).finally(() => clearTimeout(handshake));
      if (response.status === 409 && eventCursor !== null) {
        eventCursor = null;
        throw new StationConnectionError("Station event cursor expired");
      }
      if (!response.ok)
        throw new StationConnectionError(
          `Station event stream returned ${response.status}`,
        );
      await readStationEvents(
        response,
        (event) => applyStationEvent(event, connecting.signal),
        {
          milliseconds: EVENT_READ_TIMEOUT_MILLISECONDS,
          abort: () =>
            connecting.abort(
              new StationTimeoutError("Station event stream went silent"),
            ),
        },
      );
      throw new StationConnectionError("Station event stream ended");
    })()
      .catch((error: unknown) => {
        if (!connecting.signal.aborted)
          console.warn("Station event stream ended", error);
      })
      .finally(() => {
        if (eventStream !== connecting) return;
        connecting.abort();
        eventStream = null;
        latestState = null;
        if (!ready) lastError = "The station refused the connection";
        disconnectedSince ??= Date.now();
        loseLease();
        setReady(false);
        reconnectTimer = setTimeout(
          () => {
            reconnectTimer = null;
            connect();
          },
          reconnectDelay(Date.now() - attemptStartedAt),
        );
      });
  }

  function ensureConnecting(): void {
    if (closed)
      throw new StationConnectionError("The station connection is closed");
    if (eventStream === null) connect();
  }

  function noConnection(): StationConnectionError {
    return new StationConnectionError(
      `No connection to ${typeof stationUrl === "string" ? stationUrl : stationUrl()}`,
    );
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

  async function sendCommand(
    arm: string,
    command: StationCommand,
    expectedRevision?: number,
    traceContext?: TraceContext,
    signal?: AbortSignal,
    awaitCompletion = false,
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
    const station_instance_id = (await stationApi().getStationInstance()).id;
    if (signal?.aborted) throw asError(signal.reason);
    const request_id = newRequestId();
    const submission = apiSchemas.shape.CommandSubmissionRequest.parse({
      arm,
      command,
      station_instance_id,
      request_id,
      lease_token: token,
      ...(expectedRevision === undefined
        ? {}
        : { expected_revision: expectedRevision }),
      ...(traceContext ? { trace_context: traceContext } : {}),
    });
    return submitHttpCommand(
      () => stationApi().submitCommandHttp(submission),
      { request_id, station_instance_id, token, arms: [arm] },
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
  ): Promise<CommandAnswer> {
    if (signal?.aborted) throw asError(signal.reason);
    const token = heldLeaseToken();
    if (token === null) throw new LeaseLostError();
    const station_instance_id = (await stationApi().getStationInstance()).id;
    if (signal?.aborted) throw asError(signal.reason);
    const request_id = newRequestId();
    const submission = apiSchemas.shape.GroupCommandSubmissionRequest.parse({
      command,
      request_id,
      station_instance_id,
      lease_token: token,
      ...(expectedRevisions ? { expected_revisions: expectedRevisions } : {}),
      ...(traceContext ? { trace_context: traceContext } : {}),
    });
    return submitHttpCommand(
      () => stationApi().submitGroupCommandHttp(submission),
      {
        request_id,
        station_instance_id,
        token,
        arms: Object.keys(command.targets),
      },
      signal,
      awaitCompletion,
    );
  }

  async function submitHttpCommand(
    submit: () => Promise<z.infer<typeof apiSchemas.shape.CommandAnswer>>,
    identity: {
      request_id: string;
      station_instance_id: string;
      token: string | null;
      arms: string[];
    },
    signal?: AbortSignal,
    awaitCompletion = false,
  ): Promise<CommandAnswer> {
    const cancel = () => {
      if (identity.token === null || identity.arms.length === 0) return;
      void stationApi()
        .cancelCommandSubmissionHttp({
          request_id: identity.request_id,
          station_instance_id: identity.station_instance_id,
          lease_token: identity.token,
          arms: identity.arms,
        })
        .catch((error: unknown) =>
          console.warn("Could not cancel command", error),
        );
    };
    if (signal?.aborted) throw asError(signal.reason);
    let retainedCancellation = false;
    const submitAndWait = async (): Promise<CommandAnswer> => {
      const deadline = Date.now() + REPLY_TIMEOUT_MILLISECONDS;
      let response: z.infer<typeof apiSchemas.shape.CommandAnswer>;
      while (true) {
        if (signal?.aborted) throw asError(signal.reason);
        try {
          response = await submit();
          break;
        } catch (error) {
          if (signal?.aborted) throw error;
          if (error instanceof TypeError && Date.now() < deadline) {
            await new Promise((resolve) => setTimeout(resolve, 100));
            continue;
          }
          if (!(error instanceof StationHttpError)) cancel();
          throw error;
        }
      }
      const parsed = serverMessageSchema.parse({
        kind: "command_answer",
        request_id: identity.request_id,
        answer: response,
      });
      if (parsed.kind !== "command_answer")
        throw new StationConnectionError("Unexpected command response");
      if (parsed.answer.status === "accepted")
        for (const entry of parsed.answer.records) rememberRecord(entry);
      if (awaitCompletion) return finishedAnswer(parsed.answer, signal);
      if (signal && !signal.aborted && parsed.answer.status === "accepted") {
        retainedCancellation = true;
        const commandIds = parsed.answer.records.map(
          (entry) => entry.record.command_id,
        );
        void waitFor(
          () =>
            commandIds.every((id) => records.get(id)?.record.completion != null)
              ? true
              : undefined,
          { signal, patient: true },
        )
          .catch(() => {})
          .finally(() => signal.removeEventListener("abort", onAbort));
      }
      return parsed.answer;
    };
    let rejectAbort: (reason: Error) => void = () => {};
    const aborted = new Promise<never>((_resolve, reject) => {
      rejectAbort = reject;
    });
    const onAbort = () => {
      cancel();
      rejectAbort(asError(signal?.reason));
    };
    if (signal === undefined) return submitAndWait();
    signal.addEventListener("abort", onAbort, { once: true });
    try {
      return await Promise.race([submitAndWait(), aborted]);
    } finally {
      if (!retainedCancellation) signal.removeEventListener("abort", onAbort);
    }
  }

  async function getActionSequence(
    action_sequenceId: string,
  ): Promise<ActionSequenceRecord> {
    const base = typeof stationUrl === "string" ? stationUrl : stationUrl();
    return createStationApi(base, base).getActionSequence(action_sequenceId);
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
    afterFrameCount?: number,
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
        timeout: {
          milliseconds: REPLY_TIMEOUT_MILLISECONDS,
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
        const base = typeof stationUrl === "string" ? stationUrl : stationUrl();
        await stationFetch(new URL("/leases/release", base), {
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
        request_id: newRequestId(),
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
            // A holder can release between the refusal and the state read.
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

  function stationApi(): ReturnType<typeof createStationApi> {
    const base = typeof stationUrl === "string" ? stationUrl : stationUrl();
    return createStationApi(base, base);
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
      );
  }

  function runCommand(
    arm: string,
    command: StationCommand,
    expectedRevision?: number,
    signal?: AbortSignal,
    traceContext?: TraceContext,
    waitForOutcome = true,
  ): Promise<CommandAnswer> {
    return sendCommand(
      arm,
      command,
      expectedRevision,
      traceContext,
      signal,
      waitForOutcome,
    );
  }

  function runGroup(
    command: MoveArmsCommand,
    expectedRevisions?: Record<string, number>,
    signal?: AbortSignal,
    traceContext?: TraceContext,
  ): Promise<CommandAnswer> {
    return sendGroup(command, expectedRevisions, traceContext, signal, true);
  }

  async function startActionSequence(
    action_sequence: ActionSequenceRequest,
  ): Promise<string> {
    const token = heldLeaseToken();
    if (token === null) throw new LeaseLostError();
    return stationApi().startActionSequenceHttp(
      apiSchemas.shape.ActionSequenceStartRequest.parse({
        request_id: newRequestId(),
        station_instance_id: (await stationApi().getStationInstance()).id,
        lease_token: token,
        action_sequence,
      }),
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
      arm: (arm, armOptions = {}) => {
        const commands = armMotionCommands((command, revision) => {
          requireCurrentLease();
          return sendAs(arm, armOptions)(command, revision);
        });
        return {
          ...commands,
          async rehome(expectedRevision) {
            requireCurrentLease();
            const stationState = await state();
            const selected = stationState.arms.find((item) => item.arm === arm);
            if (!selected || selected.rest_pose === null)
              return {
                status: "refused",
                reason: "not_enabled",
                detail: `${arm} has no rest pose; enable it first`,
              };
            const rehomeCommands = armMotionCommands((command, revision) => {
              requireCurrentLease();
              return sendAs(arm, { ...armOptions, waitForOutcome: true })(
                command,
                revision,
              );
            });
            const rehomeSpeed = Math.min(2.0, stationState.max_joint_speed);
            const homed = await rehomeCommands.moveToJoints(
              selected.rest_pose,
              rehomeSpeed,
              expectedRevision,
            );
            if (
              homed.status !== "accepted" ||
              homed.records.some(
                ({ record }) => record.completion?.outcome !== "completed",
              )
            )
              return homed;
            return rehomeCommands.setGripper(0, rehomeSpeed, expectedRevision);
          },
        };
      },
      sendCommand(...args) {
        requireCurrentLease();
        return sendCommand(...args);
      },
      sendGroup(...args) {
        requireCurrentLease();
        return sendGroup(...args);
      },
      runCommand(...args) {
        requireCurrentLease();
        return runCommand(...args);
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
        return stationApi().runCompiledSequenceHttp({
          request_id: newRequestId(),
          station_instance_id: (await stationApi().getStationInstance()).id,
          compilation_id: compilationId,
          continuation: continuation ?? null,
          lease_token: token,
          expected_revisions: expectedRevisions,
        });
      },
      startActionSequence(actionSequence) {
        requireCurrentLease();
        return startActionSequence(actionSequence);
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
    start() {
      closed = false;
      if (eventStream === null) connect();
    },
    close() {
      closed = true;
      void returnLease(true).catch((error: unknown) =>
        console.warn("Could not release lease", error),
      );
      if (reconnectTimer !== null) clearTimeout(reconnectTimer);
      reconnectTimer = null;
      const previous = eventStream;
      eventStream = null;
      latestState = null;
      loseLease();
      setReady(false);
      previous?.abort();
    },
    waitUntilConnected,
    arm: (arm, options = {}) => armPowerCommands(sendAs(arm, options)),
    waitForOutcome,
    getActionSequence,
    async waitForActionSequence(actionSequenceId) {
      let result: ActionSequenceRecord | undefined;
      let failure: Error | undefined;
      const listener = (current: ActionSequenceRecord | null) => {
        if (
          current?.id === actionSequenceId &&
          current.phase.state !== "running"
        ) {
          result = current;
        } else if (current && current.id !== actionSequenceId) {
          void getActionSequence(actionSequenceId)
            .then((previous) => {
              if (previous.phase.state !== "running") result = previous;
              notify();
            })
            .catch((error: unknown) => {
              failure = asError(error);
              notify();
            });
        }
      };
      action_sequenceListeners.add(listener);
      try {
        if (latestActionSequence !== undefined) listener(latestActionSequence);
        return await waitFor(() => {
          if (failure !== undefined) throw failure;
          return result;
        });
      } finally {
        action_sequenceListeners.delete(listener);
      }
    },
    async cancelActionSequence(action_sequenceId) {
      try {
        await stationApi().cancelActionSequenceHttp(action_sequenceId);
      } catch (error) {
        if (!(error instanceof StationHttpError && error.status === 404))
          throw error;
      }
    },
    onActionSequence(listener) {
      action_sequenceListeners.add(listener);
      if (latestActionSequence !== undefined) listener(latestActionSequence);
      return () => {
        action_sequenceListeners.delete(listener);
      };
    },
    async preview(arm, pose) {
      const response = await previewPoses({
        arm,
        poses: [
          {
            id: newRequestId(),
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
      // The cached frame can predate the take and show no holder, so only a
      // frame received after the take's reply counts.
      const framesBeforeReturn = stateFrameCount;
      await returnLease();
      await waitForState(
        (current) => current.lease === null,
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

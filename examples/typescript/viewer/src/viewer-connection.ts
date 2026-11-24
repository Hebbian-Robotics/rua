import {
  type ActionSequenceRecord,
  type ArmCommandOptions,
  apiSchemas,
  type Lease,
  LeaseHeldElsewhereError,
  type LiveRun,
  newOperationId,
  type PreviewPose,
  type StationState,
} from "hebbian-rua";
import {
  type ManualRequest,
  manualGrant,
  viewerEvent,
} from "../shared/manual-session.js";

function subscriptions<T>() {
  const listeners = new Set<(value: T) => void>();
  return {
    subscribe(listener: (value: T) => void) {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
    notify(value: T) {
      for (const listener of listeners) listener(value);
    },
  };
}
const states = subscriptions<StationState>();
const connectivity = subscriptions<boolean>();
const leasesLost = subscriptions<void>();
const sequences = subscriptions<ActionSequenceRecord | null>();
const runs = subscriptions<LiveRun>();
let latestState: StationState | null = null;
let latestSequence: ActionSequenceRecord | null = null;
let connected = false;
let sessionId: string | null = null;
let events: EventSource | null = null;
let currentLeaseId: string | null = null;
let lastLostLeaseId: string | null = null;

async function request(
  body: ManualRequest,
  signal?: AbortSignal,
): Promise<unknown> {
  const id = sessionId;
  if (!id) throw new Error("Viewer is disconnected");
  const response = await fetch("/api/manual/request", {
    method: "POST",
    headers: { "Content-Type": "application/json", "X-Viewer-Session": id },
    body: JSON.stringify(body),
    signal,
  });
  if (id !== sessionId) throw new Error("Viewer session ended");
  if (response.status === 204) return null;
  const data: unknown = await response.json();
  if (!response.ok) {
    if (body.kind === "acquire" && response.status === 409) {
      const parsed = apiSchemas.shape.ServerMessage.safeParse({
        kind: "lease",
        request_id: null,
        answer: data,
      });
      if (
        parsed.success &&
        parsed.data.kind === "lease" &&
        parsed.data.answer.kind !== "granted"
      )
        throw new LeaseHeldElsewhereError(parsed.data.answer);
    }
    throw new Error(
      typeof data === "object" && data && "detail" in data
        ? String(data.detail)
        : `Viewer request failed (${response.status})`,
    );
  }
  return data;
}
function commandAnswer(answer: unknown) {
  const message = apiSchemas.shape.ServerMessage.parse({
    kind: "command_answer",
    request_id: null,
    answer,
  });
  if (message.kind !== "command_answer")
    throw new Error("Unexpected command response");
  return message.answer;
}
function disconnected() {
  sessionId = null;
  connected = false;
  latestState = null;
  latestSequence = null;
  currentLeaseId = null;
  leasesLost.notify();
  connectivity.notify(false);
  sequences.notify(null);
}
function closeSession() {
  events?.close();
  events = null;
  disconnected();
}

async function leaseRequest(
  body: Extract<ManualRequest, { kind: "acquire" | "return" }>,
) {
  const owner = sessionId;
  try {
    return await request(body);
  } catch (error) {
    // A lost response can leave the backend renewing a lease the tab no longer knows about.
    if (
      owner &&
      owner === sessionId &&
      !(error instanceof LeaseHeldElsewhereError)
    ) {
      closeSession();
      start();
    }
    throw error;
  }
}

function start() {
  if (events) return;
  const source = new EventSource("/api/manual/events");
  events = source;
  source.onerror = () => {
    if (events === source) disconnected();
  };
  source.onmessage = (message) => {
    if (events !== source) return;
    const event = viewerEvent.parse(JSON.parse(message.data));
    switch (event.kind) {
      case "session":
        sessionId = event.id;
        break;
      case "heartbeat":
        void request({ kind: "heartbeat" }).catch(() => {});
        break;
      case "connected":
        connected = event.connected;
        connectivity.notify(connected);
        break;
      case "leaseLost":
        lastLostLeaseId = event.leaseId;
        if (currentLeaseId === event.leaseId) {
          currentLeaseId = null;
          leasesLost.notify();
        }
        break;
      case "state":
        latestState = event.state;
        states.notify(event.state);
        break;
      case "run":
        runs.notify(event.run);
        break;
      case "sequence":
        latestSequence = event.sequence;
        sequences.notify(event.sequence);
        break;
    }
  };
}
export interface ViewerLease
  extends Pick<
    Lease,
    "leaseSeconds" | "return" | "sendCommand" | "updateStreamGoal"
  > {
  arm(
    arm: string,
    options: ArmCommandOptions,
  ): Pick<ReturnType<Lease["arm"]>, "rehome">;
}
async function acquireLease(label: string): Promise<ViewerLease> {
  const acquiringSession = sessionId;
  const { leaseId, leaseSeconds } = manualGrant.parse(
    await leaseRequest({ kind: "acquire", label }),
  );
  if (sessionId !== acquiringSession || lastLostLeaseId === leaseId)
    throw new Error("Manual lease ended during acquisition");
  currentLeaseId = leaseId;
  function motion(
    body: Extract<ManualRequest, { kind: "send" | "rehome" }>,
    signal?: AbortSignal,
  ) {
    signal?.throwIfAborted();
    // Admission can finish before motion does. Keep cancellation tied to its operation ID.
    signal?.addEventListener(
      "abort",
      () => {
        void request({
          kind: "cancel",
          leaseId,
          operationId: body.operationId,
        }).catch(() => {});
      },
      { once: true },
    );
    return request(body, signal).then(commandAnswer);
  }
  return {
    leaseSeconds,
    return: async () => {
      await leaseRequest({ kind: "return", leaseId });
      if (currentLeaseId === leaseId) currentLeaseId = null;
    },
    sendCommand: (
      arm,
      command,
      _revision,
      _trace,
      signal,
      operationId = newOperationId(),
    ) => motion({ kind: "send", arm, command, operationId, leaseId }, signal),
    updateStreamGoal: async (operationId, goal, signal) =>
      commandAnswer(
        await request({ kind: "goal", leaseId, operationId, goal }, signal),
      ),
    arm: (arm, options) => ({
      rehome: () =>
        motion(
          { kind: "rehome", arm, leaseId, operationId: newOperationId() },
          options.signal,
        ),
    }),
  };
}
export const stationConnection = {
  start,
  acquireLease,
  getState: () => latestState,
  isConnected: () => connected,
  onState: states.subscribe,
  onConnectionChange: connectivity.subscribe,
  onLeaseLost: leasesLost.subscribe,
  onRun: runs.subscribe,
  onActionSequence(listener: (sequence: ActionSequenceRecord | null) => void) {
    listener(latestSequence);
    return sequences.subscribe(listener);
  },
  async preview(arm: string, pose: PreviewPose) {
    const answer = await request({ kind: "preview", arm, pose });
    const message = apiSchemas.shape.ServerMessage.parse({
      kind: "preview_answer",
      request_id: null,
      answer,
    });
    if (message.kind !== "preview_answer")
      throw new Error("Unexpected preview response");
    return message.answer;
  },
  arm(arm: string, options: ArmCommandOptions = {}) {
    const power = async (
      action: Extract<ManualRequest, { kind: "power" }>["action"],
    ) =>
      commandAnswer(
        await request(
          {
            kind: "power",
            arm,
            action,
            waitForOutcome: options.waitForOutcome,
          },
          options.signal,
        ),
      );
    return {
      enable: () => power("enable"),
      disable: () => power("disable"),
      clearFault: () => power("clearFault"),
      approveRecovery: () => power("approveRecovery"),
      stop: () => power("stop"),
    };
  },
};
if (typeof window !== "undefined") {
  window.addEventListener("pagehide", closeSession);
  window.addEventListener("pageshow", (event) => {
    if (event.persisted) start();
  });
}

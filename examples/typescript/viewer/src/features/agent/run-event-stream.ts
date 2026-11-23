import {
  RUN_STREAM_HEARTBEAT_EVENT,
  type RunEvent,
  runEventSchema,
} from "../../../shared/run-api.js";

const STREAM_IDLE_MILLISECONDS = 30_000;

export type RunStreamRecovery = "ready" | "waiting_for_poll";
export type RunStreamEvent = "stream_lost" | "poll_succeeded";

export function runStreamRecovery(event: RunStreamEvent): RunStreamRecovery {
  return event === "stream_lost" ? "waiting_for_poll" : "ready";
}

export function runStreamId(
  activeRunId: string | null,
  recovery: RunStreamRecovery,
): string | null {
  return recovery === "ready" ? activeRunId : null;
}

export function openRunEventStream(
  url: string,
  onEvent: (event: RunEvent) => void,
  onStreamLost: () => void,
): () => void {
  const source = new EventSource(url);
  let idleDeadline: ReturnType<typeof setTimeout> | undefined;
  const listeners = new AbortController();

  function closeStream() {
    if (listeners.signal.aborted) return;
    listeners.abort();
    clearTimeout(idleDeadline);
    source.close();
  }

  function fallBackToPolling() {
    closeStream();
    onStreamLost();
  }

  function resetIdleDeadline() {
    clearTimeout(idleDeadline);
    idleDeadline = setTimeout(fallBackToPolling, STREAM_IDLE_MILLISECONDS);
  }

  resetIdleDeadline();
  const options = { signal: listeners.signal };
  source.addEventListener(
    RUN_STREAM_HEARTBEAT_EVENT,
    resetIdleDeadline,
    options,
  );
  source.addEventListener("error", fallBackToPolling, options);
  source.addEventListener(
    "message",
    (message) => {
      resetIdleDeadline();
      let payload: unknown;
      try {
        payload = JSON.parse(message.data);
      } catch {
        return;
      }
      const event = runEventSchema.safeParse(payload);
      if (!event.success) return;
      onEvent(event.data);
      if (event.data.type === "phase" && event.data.phase.state === "ended")
        closeStream();
    },
    options,
  );
  return closeStream;
}

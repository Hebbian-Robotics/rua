import { ok } from "node:assert/strict";
import { afterEach, expect, test, vi } from "vitest";
import { RUN_STREAM_HEARTBEAT_EVENT } from "../shared/run-api.js";
import {
  openRunEventStream,
  type RunStreamRecovery,
  runStreamId,
  runStreamRecovery,
} from "../src/features/agent/run-event-stream.js";

class RunEventSource extends EventTarget {
  static instances: RunEventSource[] = [];
  closed = false;

  constructor(readonly url: string) {
    super();
    RunEventSource.instances.push(this);
  }

  close(): void {
    this.closed = true;
  }
}

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
  RunEventSource.instances = [];
});

test("a quiet stream switches to polling and a successful poll reopens it", () => {
  vi.useFakeTimers();
  vi.stubGlobal("EventSource", RunEventSource);
  let recovery: RunStreamRecovery = "ready";
  function openCurrentRun() {
    const runId = runStreamId("run-1", recovery);
    return runId === null
      ? undefined
      : openRunEventStream(
          `https://agent.test/api/runs/${runId}/events`,
          () => {},
          () => {
            recovery = runStreamRecovery("stream_lost");
          },
        );
  }

  openCurrentRun();
  const [first] = RunEventSource.instances;
  ok(first, "The active run did not open a stream");
  vi.advanceTimersByTime(20_000);
  first.dispatchEvent(new Event(RUN_STREAM_HEARTBEAT_EVENT));
  vi.advanceTimersByTime(20_000);
  expect(first.closed).toBe(false);

  vi.advanceTimersByTime(10_000);
  expect(first.closed).toBe(true);
  expect(runStreamId("run-1", recovery)).toBeNull();
  expect(openCurrentRun()).toBeUndefined();

  recovery = runStreamRecovery("poll_succeeded");
  const cleanup = openCurrentRun();
  const [, reopened] = RunEventSource.instances;
  ok(reopened, "The successful poll did not reopen the stream");
  expect(reopened.url).toBe(first.url);
  expect(reopened.closed).toBe(false);
  cleanup?.();

  reopened.dispatchEvent(new Event(RUN_STREAM_HEARTBEAT_EVENT));
  vi.advanceTimersByTime(30_000);
  expect(reopened.closed).toBe(true);
  expect(recovery).toBe("ready");
});

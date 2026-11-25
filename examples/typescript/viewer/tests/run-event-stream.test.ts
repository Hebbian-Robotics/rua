import { ok } from "node:assert/strict";
import { afterEach, expect, test, vi } from "vitest";
import { RUN_STREAM_HEARTBEAT_EVENT } from "../shared/run-api.js";
import { openRunEventStream } from "../src/features/agent/run-event-stream.js";

class RunEventSource extends EventTarget {
  static instances: RunEventSource[] = [];
  closed = false;

  constructor() {
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

test("heartbeats extend the stream deadline until the stream goes quiet", () => {
  vi.useFakeTimers();
  vi.stubGlobal("EventSource", RunEventSource);
  let streamLost = false;
  openRunEventStream(
    "https://agent.test/api/runs/run-1/events",
    () => {},
    () => {
      streamLost = true;
    },
  );
  const [source] = RunEventSource.instances;
  ok(source, "The active run did not open a stream");
  vi.advanceTimersByTime(20_000);
  source.dispatchEvent(new Event(RUN_STREAM_HEARTBEAT_EVENT));
  vi.advanceTimersByTime(29_999);
  expect(source.closed).toBe(false);
  expect(streamLost).toBe(false);

  vi.advanceTimersByTime(1);
  expect(source.closed).toBe(true);
  expect(streamLost).toBe(true);
});

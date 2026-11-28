import assert from "node:assert/strict";
import {
  createServer,
  type IncomingMessage,
  type ServerResponse,
} from "node:http";
import type { AddressInfo } from "node:net";
import { setImmediate } from "node:timers/promises";
import { expect, test, vi } from "vitest";
import {
  type ActionSequenceRecord,
  createStationConnection,
  StationTimeoutError,
} from "../src/index.js";
import { commandRecord, stateMessage } from "./station-fakes.js";

test("a cancelled acceptance-only command rejects with its abort reason", async () => {
  const connection = createStationConnection("http://127.0.0.1:1", "agent");
  const reason = new Error("Operator cancelled before sending the command");
  try {
    await expect(
      connection
        .arm("follower", {
          waitForOutcome: false,
          signal: AbortSignal.abort(reason),
        })
        .enable(),
    ).rejects.toBe(reason);
  } finally {
    connection.close();
  }
});

test("a stalled command request times out and cancels its transport", async () => {
  let notifyCommandStarted: () => void = () => {};
  const commandStarted = new Promise<void>((resolve) => {
    notifyCommandStarted = resolve;
  });
  let transportAborted = false;
  const fetchSpy = vi
    .spyOn(globalThis, "fetch")
    .mockImplementation(async (input, options) => {
      if (String(input).endsWith("/instance")) {
        return Response.json({
          id: "11111111-1111-4111-8111-111111111111",
          supports_operation_completion: true,
        });
      }
      notifyCommandStarted();
      return new Promise<Response>((_resolve, reject) => {
        options?.signal?.addEventListener(
          "abort",
          () => {
            transportAborted = true;
            reject(options.signal?.reason);
          },
          { once: true },
        );
      });
    });
  vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
  const connection = createStationConnection("http://127.0.0.1:1", "agent");
  try {
    const result = expect(
      connection.arm("follower", { waitForOutcome: false }).enable(),
    ).rejects.toBeInstanceOf(StationTimeoutError);
    await commandStarted;
    await vi.advanceTimersByTimeAsync(30_000);
    await result;
    expect(transportAborted).toBe(true);
  } finally {
    connection.close();
    vi.useRealTimers();
    fetchSpy.mockRestore();
  }
});

test("a silent event stream reconnects and refreshes an expired cursor immediately", async () => {
  const cursor = "11111111-1111-4111-8111-111111111111:1";
  const requestedCursors: (string | string[] | undefined)[] = [];
  let onFreshSnapshotRequest: () => void = () => {};
  const freshSnapshotRequested = new Promise<void>((resolve) => {
    onFreshSnapshotRequest = resolve;
  });
  const server = createServer((request, response) => {
    if (request.url?.startsWith("/run/records?")) {
      response.writeHead(200, { "Content-Type": "application/json" }).end("[]");
      return;
    }
    requestedCursors.push(request.headers["last-event-id"]);
    if (requestedCursors.length === 2) {
      response.writeHead(409).end();
      return;
    }
    response.writeHead(200, { "Content-Type": "text/event-stream" });
    response.write(
      `id: ${cursor}\nevent: snapshot\ndata: ${JSON.stringify({ run_sequence: 0, items: [stateMessage([{}])] })}\n\n`,
    );
    if (requestedCursors.length === 3) onFreshSnapshotRequest();
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const address = server.address() as AddressInfo;
  const connection = createStationConnection(
    `http://127.0.0.1:${address.port}`,
    "agent",
  );
  vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "Date"] });
  try {
    connection.start();
    await connection.waitUntilConnected();
    await vi.advanceTimersByTimeAsync(30_000);
    await freshSnapshotRequested;
    expect(requestedCursors).toEqual([undefined, cursor, undefined]);
    await connection.waitUntilConnected();
  } finally {
    connection.close();
    vi.useRealTimers();
    server.closeAllConnections();
    await new Promise<void>((resolve, reject) =>
      server.close((error) => (error ? reject(error) : resolve())),
    );
  }
});

test("closing and restarting releases the old event stream", async () => {
  let requestCount = 0;
  const tracingHeaders: (string | string[] | undefined)[] = [];
  let notifyFirstStreamClosed: () => void = () => {};
  const firstStreamClosed = new Promise<void>((resolve) => {
    notifyFirstStreamClosed = resolve;
  });
  const server = createServer((request, response) => {
    if (request.url?.startsWith("/run/records?")) {
      response.writeHead(200, { "Content-Type": "application/json" }).end("[]");
      return;
    }
    tracingHeaders.push(request.headers["traceparent"], request.headers["b3"]);
    requestCount += 1;
    const currentRequest = requestCount;
    if (currentRequest === 1) response.on("close", notifyFirstStreamClosed);
    response.writeHead(200, { "Content-Type": "text/event-stream" });
    response.write(
      `event: snapshot\ndata: ${JSON.stringify({ run_sequence: 0, items: [stateMessage([{ revision: currentRequest }])] })}\n\n`,
    );
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const address = server.address() as AddressInfo;
  const connection = createStationConnection(
    `http://127.0.0.1:${address.port}`,
    "agent",
  );
  try {
    connection.start();
    await connection.waitForState((state) => state.arms[0]?.revision === 1);
    connection.close();
    connection.start();
    await firstStreamClosed;
    const state = await connection.waitForState(
      (current) => current.arms[0]?.revision === 2,
    );
    expect(state.arms[0]?.revision).toBe(2);
    expect(requestCount).toBe(2);
    expect(tracingHeaders).toEqual([
      undefined,
      undefined,
      undefined,
      undefined,
    ]);
  } finally {
    connection.close();
    server.closeAllConnections();
    await new Promise<void>((resolve, reject) =>
      server.close((error) => (error ? reject(error) : resolve())),
    );
  }
});

test("an aborted state wait stops observing later station updates", async () => {
  let stream: ServerResponse | undefined;
  const server = createServer((_request, response) => {
    stream = response;
    response.writeHead(200, { "Content-Type": "text/event-stream" });
    response.write(
      `event: snapshot\ndata: ${JSON.stringify({ run_sequence: 0, items: [stateMessage([{ revision: 1 }])] })}\n\n`,
    );
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const address = server.address() as AddressInfo;
  const connection = createStationConnection(
    `http://127.0.0.1:${address.port}`,
    "agent",
  );
  const controller = new AbortController();
  let evaluations = 0;
  try {
    await connection.waitUntilConnected();
    await expect(
      connection.waitForState((state) => state.arms[0]?.revision === 1, {
        timeoutMilliseconds: 0,
      }),
    ).resolves.toBeDefined();
    const waiting = connection.waitForState(
      () => {
        evaluations += 1;
        return false;
      },
      { signal: controller.signal },
    );
    const rejected = expect(waiting).rejects.toThrow("Stop waiting");
    controller.abort(new Error("Stop waiting"));
    await rejected;
    const evaluationsAfterAbort = evaluations;
    stream?.write(
      `event: station\ndata: ${JSON.stringify(stateMessage([{ revision: 2 }]))}\n\n`,
    );
    await connection.waitForState((state) => state.arms[0]?.revision === 2);
    expect(evaluationsAfterAbort).toBeGreaterThan(0);
    expect(evaluations).toBe(evaluationsAfterAbort);
  } finally {
    connection.close();
    server.closeAllConnections();
    await new Promise<void>((resolve, reject) =>
      server.close((error) => (error ? reject(error) : resolve())),
    );
  }
});

test("replays retained run records before publishing the snapshot", async () => {
  const delivered: string[] = [];
  const server = createServer((request, response) => {
    if (request.url?.startsWith("/run/records?")) {
      response.writeHead(200, { "Content-Type": "application/json" }).end(
        JSON.stringify([
          {
            kind: "run_event",
            seq: 1,
            received_unix_nano: "1",
            event: { label: "first" },
          },
          {
            kind: "run_event",
            seq: 1,
            received_unix_nano: "1",
            event: { label: "duplicate" },
          },
          {
            kind: "run_event",
            seq: 2,
            received_unix_nano: "2",
            event: { label: "second" },
          },
        ]),
      );
      return;
    }
    response.writeHead(200, { "Content-Type": "text/event-stream" });
    response.write(
      `event: snapshot\ndata: ${JSON.stringify({ run_sequence: 2, items: [stateMessage([{ revision: 1 }])] })}\n\n`,
    );
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const address = server.address() as AddressInfo;
  const connection = createStationConnection(
    `http://127.0.0.1:${address.port}`,
    "agent",
    { runAfter: 0 },
  );
  connection.onRunRecord((record) => {
    if (record.kind === "run_event") delivered.push(`run:${record.seq}`);
  });
  connection.onState(() => delivered.push("state"));
  try {
    connection.start();
    await connection.waitForState((state) => state.arms[0]?.revision === 1);
    expect(delivered).toEqual(["run:1", "run:2", "state"]);
  } finally {
    connection.close();
    server.closeAllConnections();
    await new Promise<void>((resolve, reject) =>
      server.close((error) => (error ? reject(error) : resolve())),
    );
  }
});

test("restarting during replay cancels the old request and publishes only the new snapshot", async () => {
  let eventRequests = 0;
  let notifyReplayStarted: () => void = () => {};
  let notifyReplayClosed: () => void = () => {};
  const replayStarted = new Promise<void>((resolve) => {
    notifyReplayStarted = resolve;
  });
  const replayClosed = new Promise<void>((resolve) => {
    notifyReplayClosed = resolve;
  });
  const server = createServer((request, response) => {
    if (request.url?.startsWith("/run/records?")) {
      response.on("close", notifyReplayClosed);
      notifyReplayStarted();
      return;
    }
    eventRequests += 1;
    const revision = eventRequests;
    response.writeHead(200, {
      "Content-Type": "text/event-stream",
      ...(revision === 2 ? { "X-RUA-Run-Cursor": "new-instance:0" } : {}),
    });
    response.write(
      `event: snapshot\ndata: ${JSON.stringify({ run_sequence: 0, items: [stateMessage([{ revision }])] })}\n\n`,
    );
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const address = server.address() as AddressInfo;
  const connection = createStationConnection(
    `http://127.0.0.1:${address.port}`,
    "agent",
    { runAfter: 0 },
  );
  const revisions: (number | undefined)[] = [];
  connection.onState((state) => revisions.push(state.arms[0]?.revision));
  try {
    connection.start();
    await replayStarted;
    connection.close();
    connection.start();
    await replayClosed;
    await connection.waitForState((state) => state.arms[0]?.revision === 2);
    expect(revisions).toEqual([2]);
  } finally {
    connection.close();
    server.closeAllConnections();
    await new Promise<void>((resolve, reject) =>
      server.close((error) => (error ? reject(error) : resolve())),
    );
  }
});

test("a malformed snapshot never publishes its state before reconnecting", async () => {
  const cursor = "11111111-1111-4111-8111-111111111111:1";
  let sentMalformedSnapshot = false;
  const server = createServer((_request, response) => {
    const items = sentMalformedSnapshot
      ? [{ kind: "future_event" }, stateMessage([{ revision: 2 }])]
      : [stateMessage([{ revision: 1 }]), { kind: "state", state: null }];
    sentMalformedSnapshot = true;
    response.writeHead(200, { "Content-Type": "text/event-stream" });
    response.write(
      `id: ${cursor}\nevent: snapshot\ndata: ${JSON.stringify({ run_sequence: 0, items })}\n\n`,
    );
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const address = server.address() as AddressInfo;
  const connection = createStationConnection(
    `http://127.0.0.1:${address.port}`,
    "agent",
  );
  const revisions: (number | undefined)[] = [];
  connection.onState((state) => revisions.push(state.arms[0]?.revision));
  const disconnected = new Promise<void>((resolve) => {
    connection.onConnectionChange((connected) => {
      if (!connected) resolve();
    });
  });
  try {
    connection.start();
    await disconnected;
    expect(revisions).toEqual([]);
    const state = await connection.waitForState(
      (state) => state.arms[0]?.revision === 2,
    );
    expect(state.arms[0]?.revision).toBe(2);
    expect(revisions).toEqual([2]);
  } finally {
    connection.close();
    server.closeAllConnections();
    await new Promise<void>((resolve, reject) =>
      server.close((error) => (error ? reject(error) : resolve())),
    );
  }
});

test("a handshake timeout backs off even when a listener asks to reconnect", async () => {
  let signalFirstRequest: () => void = () => {};
  let signalRetry: (startedAt: number) => void = () => {};
  let signalDisconnect: () => void = () => {};
  let signalSecondDisconnect: () => void = () => {};
  const firstRequest = new Promise<void>((resolve) => {
    signalFirstRequest = resolve;
  });
  const retryStarted = new Promise<number>((resolve) => {
    signalRetry = resolve;
  });
  const disconnected = new Promise<void>((resolve) => {
    signalDisconnect = resolve;
  });
  const disconnectedAgain = new Promise<void>((resolve) => {
    signalSecondDisconnect = resolve;
  });
  let firstAttempt = true;
  let disconnects = 0;
  const server = createServer((request, response) => {
    if (request.url !== "/events") {
      response.writeHead(404).end();
      return;
    }
    if (firstAttempt) {
      firstAttempt = false;
      signalFirstRequest();
    } else {
      signalRetry(Date.now());
      response.writeHead(503).end();
    }
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const address = server.address() as AddressInfo;
  const connection = createStationConnection(
    `http://127.0.0.1:${address.port}`,
    "agent",
  );
  vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "Date"] });
  const unsubscribe = connection.onConnectionChange((connected) => {
    if (connected) return;
    connection.start();
    disconnects += 1;
    if (disconnects === 1) signalDisconnect();
    else signalSecondDisconnect();
  });
  try {
    connection.start();
    await firstRequest;
    await vi.advanceTimersByTimeAsync(10_000);
    await disconnected;
    const timedOutAt = Date.now();
    await vi.advanceTimersByTimeAsync(99);
    const earlyRequest = await Promise.race([
      retryStarted,
      setImmediate().then(() => undefined),
    ]);
    expect(earlyRequest).toBeUndefined();
    await vi.advanceTimersByTimeAsync(101);
    await expect(retryStarted).resolves.toBeGreaterThanOrEqual(
      timedOutAt + 100,
    );
    await disconnectedAgain;
  } finally {
    unsubscribe();
    connection.close();
    vi.useRealTimers();
    server.closeAllConnections();
    await new Promise<void>((resolve, reject) =>
      server.close((error) => (error ? reject(error) : resolve())),
    );
  }
});

test("an action-sequence wait can expire or be aborted", async () => {
  const connection = createStationConnection("http://127.0.0.1:1", "agent");
  const cancellation = new AbortController();
  try {
    const cancelled = expect(
      connection.waitForActionSequence("sequence", {
        signal: cancellation.signal,
      }),
    ).rejects.toThrow("Run canceled");
    cancellation.abort(new Error("Run canceled"));
    await cancelled;
    await expect(
      connection.waitForActionSequence("sequence", { timeoutMilliseconds: 5 }),
    ).rejects.toThrow("Timed out waiting for action sequence sequence");
  } finally {
    connection.close();
  }
});

test("aborting an action-sequence wait interrupts a network retry delay", async () => {
  let notifyFailedRequest: () => void = () => {};
  const failedRequest = new Promise<void>((resolve) => {
    notifyFailedRequest = resolve;
  });
  const fetchSpy = vi
    .spyOn(globalThis, "fetch")
    .mockImplementation(async () => {
      notifyFailedRequest();
      throw new TypeError("Network unavailable");
    });
  vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
  const cancellation = new AbortController();
  const reason = new Error("Run canceled during retry");
  const connection = createStationConnection("http://127.0.0.1:1", "agent");
  try {
    const result = connection
      .waitForActionSequence("sequence", {
        signal: cancellation.signal,
      })
      .then(
        () => undefined,
        (error: unknown) => error,
      );
    await failedRequest;
    await setImmediate();
    cancellation.abort(reason);
    const actual = await Promise.race([
      result,
      (async () => {
        for (let turn = 0; turn < 10; turn += 1) await setImmediate();
        return undefined;
      })(),
    ]);
    expect(actual).toBe(reason);
  } finally {
    connection.close();
    vi.useRealTimers();
    fetchSpy.mockRestore();
  }
});

test.each(["before", "after"] as const)(
  "action-sequence waits started %s closing reject instead of hanging",
  async (timing) => {
    const connection = createStationConnection("http://127.0.0.1:1", "agent");
    if (timing === "after") connection.close();
    const rejected = expect(
      connection.waitForActionSequence("sequence"),
    ).rejects.toThrow(/connection.*closed/);
    if (timing === "before") connection.close();
    await rejected;
  },
);

test.each(["still running", "retained", "close during wait"] as const)(
  "action-sequence waits handle %s results",
  async (delivery) => {
    const completed: ActionSequenceRecord = {
      arms: ["follower"],
      id: "sequence",
      phase: { state: "completed" },
      requested_steps: [
        {
          arms: {
            follower: {
              goal: { kind: "joints", joints: [1], max_joint_speed: null },
              waypoints: [{ at: { kind: "joints", joints: [0.5] } }],
              gripper_opening: null,
              max_joint_speed: null,
            },
          },
        },
      ],
      compilation_id: "compilation",
      continuation: null,
      steps: [],
    };
    let sentPending = false;
    let lookupRequested: (() => void) | undefined;
    const requested = new Promise<void>((resolve) => {
      lookupRequested = resolve;
    });
    const server = createServer((request, response) => {
      if (request.url === "/action_sequence/sequence/completion") {
        lookupRequested?.();
        if (delivery === "close during wait") return;
        const body =
          delivery === "still running" && !sentPending ? null : completed;
        sentPending = true;
        response
          .writeHead(200, { "Content-Type": "application/json" })
          .end(JSON.stringify(body));
      } else response.writeHead(404).end();
    });
    await new Promise<void>((resolve) =>
      server.listen(0, "127.0.0.1", resolve),
    );
    const address = server.address() as AddressInfo;
    const connection = createStationConnection(
      `http://127.0.0.1:${address.port}`,
      "agent",
    );
    try {
      const waiting = connection.waitForActionSequence("sequence");
      if (delivery === "close during wait") {
        const rejected = expect(waiting).rejects.toThrow(/connection.*closed/);
        await requested;
        connection.close();
        await rejected;
      } else {
        await expect(waiting).resolves.toEqual(completed);
      }
    } finally {
      connection.close();
      server.closeAllConnections();
      await new Promise<void>((resolve, reject) =>
        server.close((error) => (error ? reject(error) : resolve())),
      );
    }
  },
);

test("HTTP completion waits for the station's state cursor to reach the cache", async () => {
  const instance = "11111111-1111-4111-8111-111111111111";
  let stream: ServerResponse | undefined;
  let completionReceived: (() => void) | undefined;
  const received = new Promise<void>((resolve) => {
    completionReceived = resolve;
  });
  let supportsCompletion = false;
  const commandId = "completed-command";
  const publish = (sequence: number, kind: string, data: unknown) =>
    stream?.write(
      `id: ${instance}:${sequence}\nevent: ${kind}\ndata: ${JSON.stringify(data)}\n\n`,
    );
  const server = createServer((request, response) => {
    const json = (body: unknown) =>
      response
        .writeHead(200, { "Content-Type": "application/json" })
        .end(JSON.stringify(body));
    if (request.url === "/events") {
      stream = response;
      response.writeHead(200, { "Content-Type": "text/event-stream" });
      publish(1, "snapshot", {
        station_instance_id: instance,
        sequence: 1,
        run_sequence: 0,
        items: [stateMessage([{}])],
      });
    } else if (request.url === "/instance") {
      json({ id: instance, supports_operation_completion: supportsCompletion });
    } else if (request.url === "/commands") {
      request.resume();
      json({
        status: "accepted",
        records: [{ arm: "follower", record: commandRecord(commandId) }],
      });
    } else if (request.url?.endsWith("/completion")) {
      request.resume();
      json({
        records: [{ arm: "follower", record: commandRecord(commandId, 2) }],
        state_sequence: "3",
      });
      completionReceived?.();
    } else response.writeHead(404).end();
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const address = server.address() as AddressInfo;
  const connection = createStationConnection(
    `http://127.0.0.1:${address.port}`,
    "agent",
  );
  try {
    await connection.waitUntilConnected();
    await expect(connection.arm("follower").enable()).rejects.toThrow(
      "Update the station",
    );
    supportsCompletion = true;
    let finished = false;
    const command = connection
      .arm("follower")
      .enable()
      .then((answer) => {
        finished = true;
        return answer;
      });
    await received;
    publish(2, "station", stateMessage([{ sampledAt: 2 }]));
    await connection.waitForState(
      (state) => state.arms[0]?.sampled_unix_nano === "2000000000",
    );
    await setImmediate();
    expect(finished).toBe(false);
    publish(
      3,
      "station",
      stateMessage([{ sampledAt: 3, lastCommandId: commandId }]),
    );
    const answer = await command;
    assert.equal(answer.status, "accepted");
    expect(answer.records[0]?.record.phase.kind).toBe("finished");
    expect(connection.getState()?.arms[0]?.last_command?.command_id).toBe(
      commandId,
    );
  } finally {
    connection.close();
    server.closeAllConnections();
    await new Promise<void>((resolve, reject) =>
      server.close((error) => (error ? reject(error) : resolve())),
    );
  }
});

test.each(["reconnection", "close"] as const)(
  "a lease granted after %s is released",
  async (interruption) => {
    const instance = "11111111-1111-4111-8111-111111111111";
    let eventStream: ServerResponse | undefined;
    let leaseResponse: ServerResponse | undefined;
    let leaseRequested: (() => void) | undefined;
    let leaseReleased: (() => void) | undefined;
    const requested = new Promise<void>((resolve) => {
      leaseRequested = resolve;
    });
    const released = new Promise<void>((resolve) => {
      leaseReleased = resolve;
    });
    const server = createServer((request, response) => {
      if (request.url === "/events") {
        eventStream = response;
        response.writeHead(200, { "Content-Type": "text/event-stream" });
        response.write(
          `id: ${instance}:1\nevent: snapshot\ndata: ${JSON.stringify({ station_instance_id: instance, sequence: 1, run_sequence: 0, items: [stateMessage([{}])] })}\n\n`,
        );
      } else if (request.url === "/instance") {
        response.writeHead(200, { "Content-Type": "application/json" }).end(
          JSON.stringify({
            id: instance,
            supports_operation_completion: true,
          }),
        );
      } else if (request.url === "/leases") {
        leaseResponse = response;
        leaseRequested?.();
      } else if (request.url === "/leases/renew") {
        response.writeHead(204).end();
      } else if (request.url === "/leases/release") {
        response.writeHead(204).end();
        leaseReleased?.();
      } else if (request.url?.startsWith("/run/records")) {
        response
          .writeHead(200, { "Content-Type": "application/json" })
          .end("[]");
      } else {
        response.writeHead(404).end();
      }
    });
    await new Promise<void>((resolve) =>
      server.listen(0, "127.0.0.1", resolve),
    );
    const address = server.address() as AddressInfo;
    const connection = createStationConnection(
      `http://127.0.0.1:${address.port}`,
      "agent",
    );
    try {
      connection.start();
      await connection.waitUntilConnected();
      const acquiring = connection.acquireLease("test");
      const rejected = expect(acquiring).rejects.toThrow(
        "lost while acquiring",
      );
      await requested;
      if (interruption === "close") {
        connection.close();
      } else {
        let disconnected = false;
        const reconnected = new Promise<void>((resolve) => {
          const unsubscribe = connection.onConnectionChange((connected) => {
            if (!connected) disconnected = true;
            if (connected && disconnected) {
              unsubscribe();
              resolve();
            }
          });
        });
        eventStream?.destroy();
        await reconnected;
      }
      leaseResponse?.writeHead(200, { "Content-Type": "application/json" }).end(
        JSON.stringify({
          lease_token: "token",
          lease_seconds: 15,
          status: {
            holder: "agent",
            label: "test",
            required_cameras: [],
            on_interruption: "wait_for_approval",
            expires_in_seconds: 15,
          },
        }),
      );
      await rejected;
      await released;
    } finally {
      connection.close();
      server.closeAllConnections();
      await new Promise<void>((resolve, reject) =>
        server.close((error) => (error ? reject(error) : resolve())),
      );
    }
  },
);

async function startLeaseStation(
  leaseSeconds: number,
  handleRequest: (request: IncomingMessage, response: ServerResponse) => void,
) {
  const instance = "11111111-1111-4111-8111-111111111111";
  const server = createServer((request, response) => {
    if (request.url === "/events") {
      response.writeHead(200, { "Content-Type": "text/event-stream" });
      response.write(
        `id: ${instance}:1\nevent: snapshot\ndata: ${JSON.stringify({ station_instance_id: instance, sequence: 1, run_sequence: 0, items: [stateMessage([{}])] })}\n\n`,
      );
    } else if (request.url === "/instance") {
      response
        .writeHead(200, { "Content-Type": "application/json" })
        .end(
          JSON.stringify({ id: instance, supports_operation_completion: true }),
        );
    } else if (request.url === "/leases") {
      response.writeHead(200, { "Content-Type": "application/json" }).end(
        JSON.stringify({
          lease_token: "token",
          lease_seconds: leaseSeconds,
          status: {
            holder: "agent",
            label: "test",
            required_cameras: [],
            on_interruption: "wait_for_approval",
            expires_in_seconds: leaseSeconds,
          },
        }),
      );
    } else if (request.url?.startsWith("/run/records")) {
      response.writeHead(200, { "Content-Type": "application/json" }).end("[]");
    } else handleRequest(request, response);
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const address = server.address() as AddressInfo;
  const connection = createStationConnection(
    `http://127.0.0.1:${address.port}`,
    "agent",
  );
  return {
    connection,
    close: async () => {
      connection.close();
      server.closeAllConnections();
      await new Promise<void>((resolve, reject) =>
        server.close((error) => (error ? reject(error) : resolve())),
      );
    },
  };
}

test("aborting an accepted command cancels it after a completion request fails", async () => {
  const controller = new AbortController();
  let receiveCancellation: (body: unknown) => void = () => {};
  const cancelling = new Promise<unknown>((resolve) => {
    receiveCancellation = resolve;
  });
  let acceptedRequest: unknown;
  let completionFailed = false;
  let notifyCompletionRetry: () => void = () => {};
  const retriedCompletion = new Promise<void>((resolve) => {
    notifyCompletionRetry = resolve;
  });
  const { connection, close } = await startLeaseStation(
    15,
    (request, response) => {
      if (request.url?.endsWith("/completion")) {
        request.resume();
        if (completionFailed) {
          notifyCompletionRetry();
        } else {
          completionFailed = true;
          response.writeHead(503).end();
        }
        return;
      }
      if (request.url === "/commands" || request.url?.endsWith("/cancel")) {
        request.setEncoding("utf8");
        let body = "";
        request.on("data", (chunk: string) => {
          body += chunk;
        });
        request.on("end", () => {
          const payload: unknown = JSON.parse(body);
          if (request.url?.endsWith("/cancel")) {
            receiveCancellation({ path: request.url, body: payload });
            response.writeHead(204).end();
          } else {
            acceptedRequest = payload;
            response.writeHead(200, { "Content-Type": "application/json" }).end(
              JSON.stringify({
                status: "accepted",
                records: [{ arm: "follower", record: commandRecord("motion") }],
              }),
            );
          }
        });
      } else response.writeHead(204).end();
    },
  );
  try {
    const lease = await connection.acquireLease("test");
    const answer = await lease
      .arm("follower", {
        waitForOutcome: false,
        signal: controller.signal,
      })
      .streamJoints([0, 0, 0, 0, 0]);
    expect(answer.status).toBe("accepted");
    assert.ok(acceptedRequest !== null && typeof acceptedRequest === "object");
    assert.ok(
      "operation_id" in acceptedRequest &&
        "station_instance_id" in acceptedRequest,
    );
    await retriedCompletion;
    controller.abort();
    await expect(cancelling).resolves.toEqual({
      path: `/operations/${acceptedRequest.operation_id}/cancel`,
      body: {
        station_instance_id: acceptedRequest.station_instance_id,
        lease_token: "token",
      },
    });
  } finally {
    await close();
  }
});

test("a lease grant is released when its initial renewal fails", async () => {
  const releaseRequests: unknown[] = [];
  const { connection, close } = await startLeaseStation(
    15,
    (request, response) => {
      if (request.method === "POST" && request.url === "/leases/renew") {
        response.writeHead(503).end("renewal failed");
      } else if (
        request.method === "POST" &&
        request.url === "/leases/release"
      ) {
        request.setEncoding("utf8");
        let body = "";
        request.on("data", (chunk: string) => {
          body += chunk;
        });
        request.on("end", () => {
          releaseRequests.push(JSON.parse(body) as unknown);
          response.writeHead(204).end();
        });
      } else {
        response.writeHead(404).end();
      }
    },
  );
  try {
    connection.start();
    await connection.waitUntilConnected();
    await expect(connection.acquireLease("test")).rejects.toMatchObject({
      status: 503,
      body: "renewal failed",
    });
    expect(releaseRequests).toEqual([{ lease_token: "token" }]);
  } finally {
    await close();
  }
});

test("renewal failure invalidates a held lease", async () => {
  let renewalCount = 0;
  const { connection, close } = await startLeaseStation(
    0.3,
    (request, response) => {
      if (request.url !== "/leases/renew") {
        response.writeHead(404).end();
        return;
      }
      renewalCount += 1;
      response.writeHead(renewalCount === 1 ? 204 : 503).end();
    },
  );
  try {
    connection.start();
    await connection.waitUntilConnected();
    const lease = await connection.acquireLease("test");
    expect(lease.isHeld()).toBe(true);
    await vi.waitFor(() => expect(lease.isHeld()).toBe(false));
    expect(() => lease.arm("follower").streamJoints([0, 0, 0, 0, 0])).toThrow();
  } finally {
    await close();
  }
});

test("returning a lease interrupts an in-flight renewal", async () => {
  let renewalCount = 0;
  let notifyRenewalStarted: () => void = () => {};
  let notifyRenewalClosed: () => void = () => {};
  const renewalStarted = new Promise<void>((resolve) => {
    notifyRenewalStarted = resolve;
  });
  const renewalClosed = new Promise<void>((resolve) => {
    notifyRenewalClosed = resolve;
  });
  const { connection, close } = await startLeaseStation(
    0.3,
    (request, response) => {
      if (request.url === "/leases/renew") {
        renewalCount += 1;
        if (renewalCount === 1) response.writeHead(204).end();
        else {
          response.on("close", notifyRenewalClosed);
          notifyRenewalStarted();
        }
      } else if (request.url === "/leases/release") {
        response.writeHead(204).end();
      } else {
        response.writeHead(404).end();
      }
    },
  );
  try {
    connection.start();
    const lease = await connection.acquireLease("test");
    await renewalStarted;
    await lease.return();
    await renewalClosed;
    expect(lease.isHeld()).toBe(false);
  } finally {
    await close();
  }
});

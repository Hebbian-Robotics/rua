import { afterEach, expect, test, vi } from "vitest";
import {
  createRobotTools,
  createStationConnection,
  type Lease,
  LeaseLostError,
  type StationConnection,
  type StationState,
} from "../src/index.js";
import { fakeLease } from "./station-fakes.js";

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

type Mode = "holding" | "disconnected" | "faulted";

/** A connection whose state is whatever the test sets, and wakes state waiters on demand. */
function steerableConnection() {
  const connection: StationConnection = createStationConnection(
    "http://station",
    "agent",
  );
  let mode: Mode = "holding";
  let interlocks: { kind: string }[] = [];
  const listeners = new Set<(state: StationState) => void>();
  const currentState = () =>
    ({
      robot: "sim",
      arms: [{ arm: "follower", mode, interlocks, revision: 0 }],
    }) as StationState;
  vi.spyOn(connection, "getState").mockImplementation(currentState);
  vi.spyOn(connection, "waitForState").mockImplementation(
    async (predicate, options) => {
      const state = currentState();
      if (!options?.fresh && predicate(state)) return state;
      return new Promise((resolve) => {
        const listener = (next: StationState) => {
          if (!predicate(next)) return;
          listeners.delete(listener);
          resolve(next);
        };
        listeners.add(listener);
      });
    },
  );
  return {
    connection,
    setMode(next: Mode, nextInterlocks: { kind: string }[] = []) {
      mode = next;
      interlocks = nextInterlocks;
      for (const listener of listeners) listener(currentState());
    },
  };
}

function stubCameras() {
  vi.stubGlobal("fetch", async (input: URL | string) => {
    const url = new URL(String(input));
    if (url.pathname.startsWith("/cameras/"))
      return new Response(null, { status: 503 });
    throw new Error(`Unexpected URL ${url}`);
  });
}

function toolsFor(connection: StationConnection, lease: Lease = fakeLease()) {
  return createRobotTools({
    connection,
    lease,
    mediaUrl: "http://media",
    arm: "follower",
  });
}

test("a station command is parented to the trace context the wrapper supplies", async () => {
  const { connection } = steerableConnection();
  const runCommand = vi.fn(async () => ({
    status: "refused" as const,
    reason: "malformed" as const,
    detail: "x",
  }));
  const robot = toolsFor(connection, fakeLease({ runCommand }));
  const traceContext = { traceparent: "00-aa-bb-01", tracestate: null };
  const wrapped: string[] = [];
  await robot.execute(
    [{ id: "1", name: "stop", arguments: {} }],
    undefined,
    (call, run) => {
      wrapped.push(call.name);
      return run(traceContext);
    },
  );
  expect(wrapped).toEqual(["stop"]);
  expect(runCommand).toHaveBeenCalledWith(
    "follower",
    expect.objectContaining({ kind: "stop" }),
    undefined,
    undefined,
    traceContext,
  );
  robot.close();
});

test("waiting for arm recovery ends when the arm holds again", async () => {
  const { connection, setMode } = steerableConnection();
  setMode("disconnected", [{ kind: "arm_disconnected" }]);
  stubCameras();
  const robot = toolsFor(connection);
  const recovered = robot.waitForArmRecovery();
  let settled = false;
  void recovered.then(() => {
    settled = true;
  });
  await new Promise<void>((resolve) => setImmediate(resolve));
  expect(settled).toBe(false);
  setMode("holding", [{ kind: "recovery_approval_required" }]);
  await new Promise<void>((resolve) => setImmediate(resolve));
  expect(settled).toBe(false);
  setMode("holding");
  expect(await recovered).toBe(true);
  robot.close();
});

test("waiting for arm recovery gives up when the arm faulted", async () => {
  const faulted = steerableConnection();
  faulted.setMode("faulted");
  stubCameras();
  const robot = toolsFor(faulted.connection);
  expect(await robot.waitForArmRecovery()).toBe(false);
  robot.close();
});

test("an observation carries the note and extra lines above the state, and names missing optional cameras", async () => {
  const { connection } = steerableConnection();
  stubCameras();
  const robot = createRobotTools({
    connection,
    lease: fakeLease(),
    mediaUrl: "http://media",
    arm: "follower",
    optionalCameras: ["phone"],
  });
  const observation = await robot.observe(false, {
    note: "After the tools ran.",
    extraLines: ["Notes:", "- red block"],
  });
  const lines = observation.text.split("\n");
  expect(lines.slice(0, 2)).toEqual(["Notes:", "- red block"]);
  expect(lines[2]).toBe("After the tools ran.");
  expect(lines[3]).toBe("Selected arm: follower.");
  const stateLine = lines.find((line) => line.startsWith("Arm state: "));
  expect(stateLine).toBeDefined();
  const modelState = JSON.parse(stateLine?.slice("Arm state: ".length) ?? "{}");
  expect(modelState.robot).toBe("sim");
  expect(modelState.arms.map((arm: { arm: string }) => arm.arm)).toEqual([
    "follower",
  ]);
  expect(lines.filter((line) => line.includes('"arms"'))).toHaveLength(1);
  expect(lines.at(-1)).toBe("Optional cameras unavailable: phone");
  robot.close();
});

test("waiting for recovery after the lease was lost raises LeaseLostError", async () => {
  const { connection } = steerableConnection();
  let ended: (() => void) | undefined;
  vi.spyOn(connection, "onLeaseLost").mockImplementation((listener) => {
    ended = listener;
    return () => {};
  });
  stubCameras();
  const robot = toolsFor(connection);
  ended?.();
  await expect(robot.waitForArmRecovery()).rejects.toBeInstanceOf(
    LeaseLostError,
  );
  robot.close();
});

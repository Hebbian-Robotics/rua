import { afterEach, expect, test, vi } from "vitest";
import { createActor } from "xstate";
import { createCameraConnectionMachine } from "../src/features/cameras/camera-connection-machine.js";

afterEach(() => vi.useRealTimers());

test("camera reconnect backs off and stops retrying when disposed", async () => {
  vi.useFakeTimers();
  let attempts = 0;
  const camera = createActor(
    createCameraConnectionMachine(async () => {
      attempts += 1;
    }),
  ).start();
  expect(attempts).toBe(1);
  camera.send({ type: "FAILED" });
  await vi.advanceTimersByTimeAsync(0);
  expect(attempts).toBe(2);
  camera.send({ type: "FAILED" });
  await vi.advanceTimersByTimeAsync(249);
  expect(attempts).toBe(2);
  await vi.advanceTimersByTimeAsync(1);
  expect(attempts).toBe(3);
  camera.send({ type: "FAILED" });
  camera.stop();
  await vi.advanceTimersByTimeAsync(30000);
  expect(attempts).toBe(3);
});

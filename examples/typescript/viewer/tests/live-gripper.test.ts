import { afterEach, expect, test, vi } from "vitest";

const setGripper = vi.fn();
const submissions: { arm: string; signal?: AbortSignal }[] = [];
vi.mock("../src/manual-lease.js", () => ({
  armMotion: (arm: string, signal?: AbortSignal) => {
    submissions.push({ arm, signal });
    return { setGripper };
  },
}));

import { createLiveGripper } from "../src/live-gripper.js";

afterEach(() => {
  vi.unstubAllGlobals();
  setGripper.mockReset();
  submissions.length = 0;
});

test("release cancels the pending gripper request for the original arm", () => {
  vi.stubGlobal("window", {
    addEventListener: () => {},
    removeEventListener: () => {},
  });
  vi.stubGlobal("document", {
    hidden: false,
    addEventListener: () => {},
    removeEventListener: () => {},
  });
  let acceptCommand: ((answer: unknown) => void) | undefined;
  setGripper.mockReturnValueOnce(
    new Promise((resolve) => {
      acceptCommand = resolve;
    }),
  );
  let selectedArm = {
    arm: "follower",
    mode: "holding" as const,
    gripper_opening: 1,
    sampled_unix_nano: String(BigInt(Date.now()) * 1_000_000n),
  };
  const gripper = createLiveGripper(
    () => selectedArm,
    () => {},
  );
  gripper.setOpening(0.5);
  selectedArm = { ...selectedArm, arm: "other" };
  gripper.hold();
  acceptCommand?.({ status: "accepted" });
  expect(submissions[0]?.arm).toBe("follower");
  expect(submissions[0]?.signal?.aborted).toBe(true);
  gripper.destroy();
});

test("a restarted command on the same arm is not stopped by an earlier release", async () => {
  vi.stubGlobal("window", {
    addEventListener: () => {},
    removeEventListener: () => {},
  });
  vi.stubGlobal("document", {
    hidden: false,
    addEventListener: () => {},
    removeEventListener: () => {},
  });
  let acceptFirst: ((answer: unknown) => void) | undefined;
  setGripper.mockReturnValueOnce(
    new Promise((resolve) => {
      acceptFirst = resolve;
    }),
  );
  setGripper.mockResolvedValue({ status: "accepted" });
  const arm = {
    arm: "follower",
    mode: "holding" as const,
    gripper_opening: 1,
    sampled_unix_nano: String(BigInt(Date.now()) * 1_000_000n),
  };
  const gripper = createLiveGripper(
    () => arm,
    () => {},
  );
  gripper.setOpening(0.5);
  gripper.hold();
  gripper.setOpening(0.8);
  acceptFirst?.({ status: "accepted" });
  await vi.waitFor(() => expect(setGripper).toHaveBeenCalledTimes(2));
  expect(submissions[0]?.signal?.aborted).toBe(true);
  expect(submissions[1]?.signal?.aborted).toBe(false);
  gripper.destroy();
});

test("losing focus cancels a gripper request that is still in flight", () => {
  let blur: (() => void) | undefined;
  vi.stubGlobal("window", {
    addEventListener: (name: string, listener: () => void) => {
      if (name === "blur") blur = listener;
    },
    removeEventListener: () => {},
  });
  vi.stubGlobal("document", {
    hidden: false,
    addEventListener: () => {},
    removeEventListener: () => {},
  });
  let acceptCommand: ((answer: unknown) => void) | undefined;
  setGripper.mockReturnValueOnce(
    new Promise((resolve) => {
      acceptCommand = resolve;
    }),
  );
  const arm = {
    arm: "follower",
    mode: "holding" as const,
    gripper_opening: 1,
    sampled_unix_nano: String(BigInt(Date.now()) * 1_000_000n),
  };
  const gripper = createLiveGripper(
    () => arm,
    () => {},
  );
  gripper.setOpening(0.5);
  blur?.();
  acceptCommand?.({ status: "accepted" });
  expect(submissions[0]?.signal?.aborted).toBe(true);
  gripper.destroy();
});

test("a queued opening for the old arm is not sent to a newly selected arm", async () => {
  vi.stubGlobal("window", {
    addEventListener: () => {},
    removeEventListener: () => {},
  });
  vi.stubGlobal("document", {
    hidden: false,
    addEventListener: () => {},
    removeEventListener: () => {},
  });
  let acceptCommand: ((answer: unknown) => void) | undefined;
  setGripper.mockReturnValueOnce(
    new Promise((resolve) => {
      acceptCommand = resolve;
    }),
  );
  setGripper.mockResolvedValue({ status: "accepted" });
  let selectedArm = {
    arm: "follower",
    mode: "holding" as const,
    gripper_opening: 1,
    sampled_unix_nano: String(BigInt(Date.now()) * 1_000_000n),
  };
  const gripper = createLiveGripper(
    () => selectedArm,
    () => {},
  );
  gripper.setOpening(0.5);
  gripper.setOpening(0.7);
  selectedArm = { ...selectedArm, arm: "other" };
  acceptCommand?.({ status: "accepted" });
  for (let turn = 0; turn < 5; turn += 1) await Promise.resolve();
  expect(setGripper).toHaveBeenCalledTimes(1);
  gripper.destroy();
});

import { afterEach, expect, test, vi } from "vitest";

const setGripper = vi.fn();
const stoppedArm = vi.fn();
const stop = vi.fn();
vi.mock("../src/manual-lease.js", () => ({
  armMotion: () => ({ setGripper }),
  armControl: (arm: string) => {
    stoppedArm(arm);
    return { stop };
  },
}));

import { createLiveGripper } from "../src/live-gripper.js";

afterEach(() => {
  vi.unstubAllGlobals();
  setGripper.mockReset();
  stoppedArm.mockReset();
  stop.mockReset();
});

test("releasing during the first pending gripper command stops its original arm", async () => {
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
  stop.mockResolvedValue({ status: "accepted" });
  let selectedArm = {
    arm: "follower",
    mode: "holding" as const,
    gripper_opening: 1,
    sampled_at_unix_seconds: Date.now() / 1000,
  };
  const gripper = createLiveGripper(
    () => selectedArm,
    () => {},
  );
  gripper.setOpening(0.5);
  selectedArm = { ...selectedArm, arm: "other" };
  const holding = gripper.hold();
  acceptCommand?.({ status: "accepted" });
  await holding;
  expect(stoppedArm).toHaveBeenCalledWith("follower");
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
    sampled_at_unix_seconds: Date.now() / 1000,
  };
  const gripper = createLiveGripper(
    () => arm,
    () => {},
  );
  gripper.setOpening(0.5);
  const holding = gripper.hold();
  gripper.setOpening(0.8);
  acceptFirst?.({ status: "accepted" });
  await holding;
  expect(setGripper).toHaveBeenCalledTimes(2);
  expect(stop).not.toHaveBeenCalled();
  gripper.destroy();
});

test("losing focus stops an in-flight gripper command after it settles", async () => {
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
  stop.mockResolvedValue({ status: "accepted" });
  const arm = {
    arm: "follower",
    mode: "holding" as const,
    gripper_opening: 1,
    sampled_at_unix_seconds: Date.now() / 1000,
  };
  const gripper = createLiveGripper(
    () => arm,
    () => {},
  );
  gripper.setOpening(0.5);
  blur?.();
  acceptCommand?.({ status: "accepted" });
  await vi.waitFor(() => expect(stoppedArm).toHaveBeenCalledWith("follower"));
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
    sampled_at_unix_seconds: Date.now() / 1000,
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

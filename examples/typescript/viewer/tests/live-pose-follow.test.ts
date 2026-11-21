import { afterEach, describe, expect, it, vi } from "vitest";

const streamJoints = vi.fn();
const submissions: {
  arm: string;
  signal?: AbortSignal;
  operationId?: string;
}[] = [];
const updateStreamGoal = vi.fn();
vi.mock("../src/manual-lease.js", () => ({
  armMotion: (arm: string, signal?: AbortSignal, operationId?: string) => {
    submissions.push({ arm, signal, operationId });
    return { streamJoints };
  },
  manualLease: {
    updateStreamGoal: (...args: unknown[]) => updateStreamGoal(...args),
  },
}));

import { createLivePoseFollow } from "../src/live-pose-follow.js";

const holdingArm = { arm: "follower", mode: "holding" as const };

afterEach(() => {
  vi.unstubAllGlobals();
  streamJoints.mockReset();
  updateStreamGoal.mockReset();
  submissions.length = 0;
});

function stubBrowserEvents(): void {
  vi.stubGlobal("window", {
    addEventListener: () => {},
    removeEventListener: () => {},
  });
  vi.stubGlobal("document", {
    hidden: false,
    addEventListener: () => {},
    removeEventListener: () => {},
  });
}

describe("createLivePoseFollow", () => {
  it("tells its owner when a refused stream disarms follow, so a checkbox can be cleared", async () => {
    stubBrowserEvents();
    streamJoints.mockResolvedValue({
      status: "refused",
      reason: "interlocked",
      detail: "no",
    });
    const reports: string[] = [];
    const onDisarmed = vi.fn();
    const follow = createLivePoseFollow(
      () => holdingArm,
      (message) => reports.push(message),
      () => 0.5,
      onDisarmed,
    );
    follow.setEnabled(true);
    follow.setDragging(true);
    follow.setGoal([0, 0, 0, 0, 0]);
    await vi.waitFor(() => expect(onDisarmed).toHaveBeenCalledTimes(1));
    expect(reports.at(-1)).toBe("Follow stopped: no");
  });

  it("repeats the newest goal while the drag is held, so the station's goal timeout never ends the stream", async () => {
    vi.useFakeTimers();
    try {
      stubBrowserEvents();
      streamJoints.mockResolvedValue({
        status: "accepted",
        records: [{ record: { command_id: "stream-1" } }],
      });
      updateStreamGoal.mockImplementation(async () => ({
        status: "accepted",
        records: [{ record: {} }],
      }));
      const follow = createLivePoseFollow(
        () => holdingArm,
        () => {},
        () => 0.5,
      );
      follow.setEnabled(true);
      follow.setDragging(true);
      follow.setGoal([0, 0, 0, 0, 0]);
      await vi.advanceTimersByTimeAsync(0);
      expect(streamJoints).toHaveBeenCalledTimes(1);
      await vi.advanceTimersByTimeAsync(1000);
      expect(updateStreamGoal.mock.calls.length).toBeGreaterThanOrEqual(4);
      const [lastStreamId, lastGoal] = updateStreamGoal.mock.calls.at(-1) ?? [];
      expect(lastGoal?.joints).toEqual([0, 0, 0, 0, 0]);
      expect(lastStreamId).toBe(submissions[0]?.operationId);
      follow.setDragging(false);
      const callsAtRelease = updateStreamGoal.mock.calls.length;
      await vi.advanceTimersByTimeAsync(1000);
      expect(updateStreamGoal.mock.calls.length).toBe(callsAtRelease);
    } finally {
      vi.useRealTimers();
    }
  });

  it("cancels an in-flight stream when the drag is released", () => {
    stubBrowserEvents();
    let acceptGoal: ((answer: unknown) => void) | undefined;
    streamJoints.mockReturnValueOnce(
      new Promise((resolve) => {
        acceptGoal = resolve;
      }),
    );
    const follow = createLivePoseFollow(
      () => holdingArm,
      () => {},
    );
    follow.setEnabled(true);
    follow.setDragging(true);
    follow.setGoal([0, 0, 0, 0, 0]);
    follow.setDragging(false);
    expect(submissions[0]?.signal?.aborted).toBe(true);
    acceptGoal?.({
      status: "accepted",
      records: [{ record: { command_id: "stream-1" } }],
    });
    expect(submissions[0]?.signal?.aborted).toBe(true);
    follow.destroy();
  });

  it("cancels the original arm stream after the selected arm changes", () => {
    stubBrowserEvents();
    let acceptGoal: ((answer: unknown) => void) | undefined;
    streamJoints.mockReturnValueOnce(
      new Promise((resolve) => {
        acceptGoal = resolve;
      }),
    );
    let selectedArm: typeof holdingArm = holdingArm;
    const follow = createLivePoseFollow(
      () => selectedArm,
      () => {},
    );
    follow.setEnabled(true);
    follow.setDragging(true);
    follow.setGoal([0, 0, 0, 0, 0]);
    selectedArm = { arm: "other", mode: "holding" };
    follow.setDragging(false);
    acceptGoal?.({
      status: "accepted",
      records: [{ record: { command_id: "stream-1" } }],
    });
    expect(submissions[0]?.arm).toBe("follower");
    expect(submissions[0]?.signal?.aborted).toBe(true);
    follow.destroy();
  });

  it("ignores an old request failure after follow has been restarted", async () => {
    stubBrowserEvents();
    let failGoal: ((reason: Error) => void) | undefined;
    streamJoints.mockReturnValueOnce(
      new Promise((_resolve, reject) => {
        failGoal = reject;
      }),
    );
    const onDisarmed = vi.fn();
    const follow = createLivePoseFollow(
      () => holdingArm,
      () => {},
      () => 0.5,
      onDisarmed,
    );
    follow.setEnabled(true);
    follow.setDragging(true);
    follow.setGoal([0, 0, 0, 0, 0]);
    follow.setEnabled(false);
    follow.setEnabled(true);
    failGoal?.(new Error("old connection failed"));
    await Promise.resolve();
    expect(submissions[0]?.signal?.aborted).toBe(true);
    expect(onDisarmed).not.toHaveBeenCalled();
    follow.destroy();
  });

  it("disarms instead of sending queued joints to a different arm", async () => {
    stubBrowserEvents();
    let acceptGoal: ((answer: unknown) => void) | undefined;
    streamJoints.mockReturnValueOnce(
      new Promise((resolve) => {
        acceptGoal = resolve;
      }),
    );
    let selectedArm: typeof holdingArm = holdingArm;
    const onDisarmed = vi.fn();
    const follow = createLivePoseFollow(
      () => selectedArm,
      () => {},
      () => 0.5,
      onDisarmed,
    );
    follow.setEnabled(true);
    follow.setDragging(true);
    follow.setGoal([0, 0, 0, 0, 0]);
    follow.setGoal([0.1, 0, 0, 0, 0]);
    selectedArm = { arm: "other", mode: "holding" };
    acceptGoal?.({
      status: "accepted",
      records: [{ record: { command_id: "stream-1" } }],
    });
    await vi.waitFor(() => expect(submissions[0]?.signal?.aborted).toBe(true));
    expect(streamJoints).toHaveBeenCalledTimes(1);
    expect(onDisarmed).toHaveBeenCalledTimes(1);
    follow.destroy();
  });

  it("stays armed when a stream ends or trips, and the next goal starts a new stream", async () => {
    stubBrowserEvents();
    streamJoints
      .mockResolvedValueOnce({
        status: "refused",
        reason: "stream_ended",
        detail: "Stream stream-1 ended (tripped: table guard)",
      })
      .mockResolvedValue({
        status: "accepted",
        records: [{ record: { command_id: "stream-2" } }],
      });
    const reports: string[] = [];
    const onDisarmed = vi.fn();
    const follow = createLivePoseFollow(
      () => holdingArm,
      (message) => reports.push(message),
      () => 0.5,
      onDisarmed,
    );
    follow.setEnabled(true);
    follow.setDragging(true);
    follow.setGoal([0, 0, 0, 0, 0]);
    await vi.waitFor(() =>
      expect(reports.at(-1)).toContain("Keep dragging to continue"),
    );
    expect(onDisarmed).not.toHaveBeenCalled();
    follow.setGoal([0.1, 0, 0, 0, 0]);
    await vi.waitFor(() => expect(streamJoints).toHaveBeenCalledTimes(2));
    expect(submissions[1]?.operationId).not.toBe(submissions[0]?.operationId);
    follow.setDragging(false);
  });
});

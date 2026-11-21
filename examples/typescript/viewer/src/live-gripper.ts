import type { ArmState } from "@hebbian/robot-use-client";
import { errorMessage } from "./errors.js";
import { createLatestOnly } from "./lib/latest-only.js";
import { armControl, armMotion } from "./manual-lease.js";

type GripperArm = Pick<
  ArmState,
  "arm" | "mode" | "sampled_at_unix_seconds" | "gripper_opening"
>;

type GripperActivity =
  | { kind: "idle" }
  | { kind: "commanded"; arm: string; acceptedOpening: number | null };

export function createLiveGripper(
  getArm: () => GripperArm | undefined,
  report: (message: string) => void,
) {
  let epoch = 0;
  let activity: GripperActivity = { kind: "idle" };

  function usableArm(): GripperArm | undefined {
    const arm = getArm();
    return arm?.gripper_opening != null &&
      (arm.mode === "holding" || arm.mode === "moving") &&
      arm.sampled_at_unix_seconds !== null &&
      Date.now() / 1000 - arm.sampled_at_unix_seconds < 0.5
      ? arm
      : undefined;
  }

  const latestOpening = createLatestOnly<{ arm: string; opening: number }>(
    async ({ arm: requestedArm, opening }) => {
      const arm = usableArm();
      if (!arm || arm.arm !== requestedArm) return;
      if (
        activity.kind === "commanded" &&
        arm.arm === activity.arm &&
        opening === activity.acceptedOpening
      )
        return;
      const sentInEpoch = epoch;
      activity = {
        kind: "commanded",
        arm: arm.arm,
        acceptedOpening:
          activity.kind === "commanded" && activity.arm === arm.arm
            ? activity.acceptedOpening
            : null,
      };
      const answer = await armMotion(arm.arm)
        .setGripper(opening)
        .catch((error: unknown) => {
          if (sentInEpoch !== epoch) return null;
          throw error;
        });
      if (sentInEpoch !== epoch || answer === null) return;
      if (answer.status === "refused") throw new Error(answer.detail);
      activity = { kind: "commanded", arm: arm.arm, acceptedOpening: opening };
    },
    (error) => report(`Gripper stopped updating: ${errorMessage(error)}`),
  );

  function cancel(): void {
    epoch += 1;
    activity = { kind: "idle" };
    latestOpening.cancel();
  }
  // SAFETY: A sent command can still be accepted after focus is lost.
  async function hold(): Promise<void> {
    const stoppedArm = activity.kind === "commanded" ? activity.arm : null;
    cancel();
    await latestOpening.idle();
    if (
      !stoppedArm ||
      (activity.kind === "commanded" && activity.arm === stoppedArm)
    )
      return;
    try {
      await armControl(stoppedArm).stop();
    } catch {
      report("Connection lost while stopping the gripper. Check the arm.");
    }
  }
  function onBlur(): void {
    void hold();
  }
  window.addEventListener("blur", onBlur);
  function onVisibilityChange(): void {
    if (document.hidden) void hold();
  }
  document.addEventListener("visibilitychange", onVisibilityChange);
  return {
    destroy(): void {
      window.removeEventListener("blur", onBlur);
      document.removeEventListener("visibilitychange", onVisibilityChange);
      cancel();
    },
    setOpening(opening: number): void {
      if (!Number.isFinite(opening) || opening < 0 || opening > 1) return;
      const arm = usableArm();
      if (!arm) {
        cancel();
        return;
      }
      latestOpening.push({ arm: arm.arm, opening });
    },
    cancel,
    hold,
  };
}

import type { ArmState } from "hebbian-rua";
import { errorMessage } from "./errors.js";
import { createLatestOnly } from "./lib/latest-only.js";
import { STALE_FEEDBACK_SECONDS } from "./live-feedback.js";
import { armMotion } from "./manual-lease.js";

type GripperArm = Pick<
  ArmState,
  "arm" | "mode" | "sampled_unix_nano" | "gripper_opening"
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
  let pending: AbortController | null = null;
  let accepted: AbortController | null = null;

  function usableArm(): GripperArm | undefined {
    const arm = getArm();
    return arm?.gripper_opening != null &&
      (arm.mode === "holding" || arm.mode === "moving") &&
      arm.sampled_unix_nano !== null &&
      Date.now() / 1000 - Number(arm.sampled_unix_nano) / 1e9 <
        STALE_FEEDBACK_SECONDS
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
      const command = new AbortController();
      pending = command;
      const answer = await armMotion(arm.arm, command.signal)
        .setGripper(opening)
        .catch((error: unknown) => {
          if (sentInEpoch !== epoch) return null;
          throw error;
        })
        .finally(() => {
          if (pending === command) pending = null;
        });
      if (sentInEpoch !== epoch || answer === null) return;
      if (answer.status === "refused") throw new Error(answer.detail);
      accepted = command;
      activity = { kind: "commanded", arm: arm.arm, acceptedOpening: opening };
    },
    (error) => report(`Gripper stopped updating: ${errorMessage(error)}`),
  );

  function cancel(): void {
    epoch += 1;
    activity = { kind: "idle" };
    latestOpening.cancel();
  }
  // ROBOT-SAFETY: Cancel both the acknowledged opening and any update still on the wire.
  function hold(): void {
    cancel();
    pending?.abort();
    pending = null;
    accepted?.abort();
    accepted = null;
  }
  function onBlur(): void {
    hold();
  }
  window.addEventListener("blur", onBlur);
  function onVisibilityChange(): void {
    if (document.hidden) hold();
  }
  document.addEventListener("visibilitychange", onVisibilityChange);
  return {
    destroy(): void {
      window.removeEventListener("blur", onBlur);
      document.removeEventListener("visibilitychange", onVisibilityChange);
      hold();
    },
    setOpening(opening: number): void {
      if (!Number.isFinite(opening) || opening < 0 || opening > 1) return;
      const arm = usableArm();
      if (!arm) {
        hold();
        return;
      }
      latestOpening.push({ arm: arm.arm, opening });
    },
    hold,
  };
}

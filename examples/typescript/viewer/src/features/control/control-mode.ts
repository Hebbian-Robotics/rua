import type { ArmState } from "hebbian-rua";

export function controlModeLockReason(
  arms: readonly Pick<ArmState, "mode">[],
  agentRunActive: boolean,
  changingArmPower: boolean,
): string | null {
  if (arms.length === 0) return "Waiting for arm status.";
  if (agentRunActive) return "End the agent run before switching modes.";
  if (changingArmPower) return "Wait for the arm power change to finish.";
  if (
    arms.some((arm) => arm.mode !== "disabled" && arm.mode !== "reconnected")
  ) {
    return arms.length === 1
      ? "Disable the arm to switch modes."
      : "Disable all arms to switch modes.";
  }
  return null;
}

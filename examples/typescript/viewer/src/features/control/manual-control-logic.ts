import type { ArmState } from "hebbian-rua";
import type { MotionLimits } from "../../../shared/run-api.js";
import { previewRequest } from "../../pose-projection.js";

export function armAcceptsManualControl(
  armMode: ArmState["mode"] | null,
): boolean {
  return armMode === "holding" || armMode === "moving";
}

export function manualMoveBlockedReason({
  ownershipReason,
  armMode,
  hasTarget,
  hasConfirmedPreview,
  projectionValidity,
}: {
  ownershipReason: string | null;
  armMode: ArmState["mode"] | null;
  hasTarget: boolean;
  hasConfirmedPreview: boolean;
  projectionValidity: boolean | null;
}): string | null {
  if (ownershipReason !== null) return ownershipReason;
  if (armMode === null) return "Waiting for robot state.";
  if (armMode !== "holding") return "Enable the arm and wait for it to hold.";
  if (!hasTarget) return "Set a goal first.";
  if (!hasConfirmedPreview) return "Waiting for a confirmed preview.";
  if (projectionValidity === null) return "Checking preview safety.";
  if (!projectionValidity)
    return "Preview blocked by the table surface or model unavailable.";
  return null;
}

// ROBOT-SAFETY: A confirmation applies only to the arm, target and orientation it solved.
export function previewMatchesSelection({
  confirmed,
  arm,
  projectionArm,
  target,
  orientation,
  motionLimits,
}: {
  confirmed: {
    arm: string;
    requestPose: import("hebbian-rua").PreviewPose;
  } | null;
  arm: string;
  projectionArm: string | null;
  target: number[] | null;
  orientation: import("../../pose-projection.js").Orientation | null;
  motionLimits?: MotionLimits;
}): boolean {
  if (!confirmed || !target || confirmed.arm !== arm || projectionArm !== arm)
    return false;
  const request = previewRequest(target, orientation, motionLimits);
  return JSON.stringify(confirmed.requestPose) === JSON.stringify(request);
}

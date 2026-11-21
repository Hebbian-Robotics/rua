import type { ArmState, StationCommand } from "@hebbian/robot-use-client";
import { MathUtils } from "three";
import type { Approach, JawYawRange, ProposedPose } from "./pose-projection.js";

// SAFETY: A one-shot move must use the previewed gripper only after its arm command is accepted.
export async function movePreviewedGripper(
  sendArmMove: () => Promise<boolean>,
  isCurrent: () => boolean,
  hasGripper: boolean,
  opening: number,
  sendGripperOpening: (opening: number) => void,
): Promise<void> {
  const accepted = await sendArmMove();
  if (
    accepted &&
    isCurrent() &&
    hasGripper &&
    Number.isFinite(opening) &&
    opening >= 0 &&
    opening <= 1
  )
    sendGripperOpening(opening);
}

// CONSTRAINT: IK moves use the ghost's solved joints; a policy preview is only a rehearsal,
// so policy moves must use the requested pose instead. No quaternion means no move.
export function moveCommandForPreview(
  controller: ArmState["controller"],
  pose: ProposedPose,
  maxJointSpeed: number,
): Extract<StationCommand, { kind: "move_to_joints" | "move_to_pose" }> | null {
  if (controller === "ik")
    return {
      kind: "move_to_joints",
      joints: pose.joints,
      max_joint_speed: maxJointSpeed,
    };
  if (!pose.requestedQuaternion) return null;
  return {
    kind: "move_to_pose",
    position: pose.requestedPosition,
    orientation: { kind: "quaternion", quaternion: pose.requestedQuaternion },
    max_joint_speed: maxJointSpeed,
  };
}

export function syncCurrentPosePreview(
  arm: Pick<ArmState, "tool" | "gripper_opening">,
  setOpening: (opening: number) => void,
  setPosition: (position: number[]) => void,
): boolean {
  if (!arm.tool) return false;
  if (arm.gripper_opening !== null)
    setOpening(MathUtils.clamp(arm.gripper_opening, 0, 1));
  setPosition(arm.tool.position);
  return true;
}

export function describeApproach(approach: Approach): string {
  if (typeof approach === "string") return approach;
  return `custom (${approach.map((value) => value.toFixed(2)).join(", ")})`;
}

function wrapDegrees(degrees: number): number {
  return ((degrees % 360) + 360) % 360;
}

export function jawYawReachable(
  ranges: JawYawRange[],
  jawYawDegrees: number,
): boolean | null {
  if (ranges.length === 0) return null;
  const yaw = wrapDegrees(jawYawDegrees);
  const containing = ranges.find(
    (range) => yaw >= range.from_degrees && yaw <= range.to_degrees,
  );
  return containing ? containing.reachable : null;
}

export function unreachableYawLines(ranges: JawYawRange[]): string[] {
  return ranges
    .filter((range) => !range.reachable)
    .map(
      (range) =>
        `${Math.round(range.from_degrees)}°–${Math.round(range.to_degrees)}°: ${range.reason ?? "unreachable"}`,
    );
}

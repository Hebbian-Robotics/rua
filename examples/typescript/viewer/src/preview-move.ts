import {
  type ArmState,
  angularDegrees,
  type PreviewPose,
  type StationCommand,
} from "hebbian-rua";
import { MathUtils } from "three";
import type { Approach, JawYawRange, ProposedPose } from "./pose-projection.js";

// ROBOT-SAFETY: A one-shot move must use the previewed gripper only after its arm command is accepted.
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
  maxJointSpeed: number | null | undefined,
  preview?: PreviewPose,
): Extract<StationCommand, { kind: "move_to_joints" | "move_to_pose" }> | null {
  if (controller === "ik")
    return {
      kind: "move_to_joints",
      joints: pose.joints,
      ...(maxJointSpeed == null ? {} : { max_joint_speed: maxJointSpeed }),
      ...(preview?.max_joint_acceleration == null
        ? {}
        : { max_joint_acceleration: preview.max_joint_acceleration }),
      ...(preview?.max_joint_jerk == null
        ? {}
        : { max_joint_jerk: preview.max_joint_jerk }),
    };
  if (!pose.requestedQuaternion) return null;
  return {
    kind: "move_to_pose",
    position: pose.requestedPosition,
    orientation: { kind: "quaternion", quaternion: pose.requestedQuaternion },
    ...(maxJointSpeed == null ? {} : { max_joint_speed: maxJointSpeed }),
  };
}

export function syncCurrentPosePreview(
  arm: Pick<ArmState, "tool" | "gripper_opening">,
  setOpening: (opening: number) => void,
  setPosition: (position: number[]) => void,
): boolean {
  const position = arm.tool?.position;
  if (
    position === undefined ||
    position.length !== 3 ||
    !position.every(Number.isFinite)
  )
    return false;
  if (arm.gripper_opening !== null)
    setOpening(MathUtils.clamp(arm.gripper_opening, 0, 1));
  setPosition(position);
  return true;
}

export function describeApproach(approach: Approach): string {
  if (typeof approach === "string") return approach;
  return `custom (${approach.map((value) => value.toFixed(2)).join(", ")})`;
}

function wrapRadians(radians: number): number {
  const fullTurn = 2 * Math.PI;
  return ((radians % fullTurn) + fullTurn) % fullTurn;
}

export function jawYawReachable(
  ranges: JawYawRange[],
  jawYawRadians: number,
): boolean | null {
  if (ranges.length === 0) return null;
  const yaw = wrapRadians(jawYawRadians);
  const containing = ranges.find(
    (range) => yaw >= range.from_radians && yaw <= range.to_radians,
  );
  return containing ? containing.reachable : null;
}

export function unreachableYawLines(ranges: JawYawRange[]): string[] {
  return ranges
    .filter((range) => !range.reachable)
    .map(
      (range) =>
        `${Math.round(angularDegrees(range.from_radians, "rad"))}°–${Math.round(angularDegrees(range.to_radians, "rad"))}°: ${range.reason ?? "unreachable"}`,
    );
}

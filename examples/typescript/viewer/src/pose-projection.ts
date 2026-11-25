import type { PoseOrientation, PoseProjection, PreviewPose } from "hebbian-rua";
import type { MotionLimits } from "../shared/run-api.js";
import { stationConnection } from "./viewer-connection.js";

export interface ProposedPose {
  joints: number[];

  position: number[];
  quaternion: number[];

  requestedPosition: number[];
  requestedQuaternion: number[] | null;
}

export type Orientation = Extract<PoseOrientation, { kind: "grasp" }>;

export type Approach = Orientation["approach"];
export type JawYawRange = PoseProjection["jaw_yaw_ranges"][number];

export function previewRequest(
  position: number[],
  orientation: Orientation | null,
  motionLimits?: MotionLimits,
): PreviewPose {
  return orientation
    ? { position, orientation, ...motionLimits }
    : { position, ...motionLimits };
}

async function askForProjection(
  arm: string,
  request: PreviewPose,
): Promise<PoseProjection> {
  const answer = await stationConnection.preview(arm, request);
  if (answer.status === "refused") throw new Error(answer.detail);
  return answer.projection;
}

function grasp(orientation: {
  approach: number[];
  jaw_yaw_radians: number;
}): Orientation {
  return {
    kind: "grasp",
    approach: orientation.approach,
    jaw_yaw_radians: orientation.jaw_yaw_radians,
  };
}

// WHY: Read approach and yaw back from the station at full precision rather than
// reconstructing them locally; resubmission must reproduce the same orientation.
export async function readCurrentOrientation(
  armName: string,
  position: number[],
  quaternion: number[],
): Promise<Orientation | null> {
  try {
    const projection = await askForProjection(armName, {
      position,
      orientation: { kind: "quaternion", quaternion },
    });
    return projection.requested
      ? grasp(projection.requested.orientation)
      : null;
  } catch {
    return null;
  }
}

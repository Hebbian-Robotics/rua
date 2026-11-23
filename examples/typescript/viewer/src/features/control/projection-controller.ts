import type { PoseProjection, PreviewAnswer, PreviewPose } from "hebbian-rua";
import { errorMessage } from "../../errors.js";
import { createLatestOnly } from "../../lib/latest-only.js";

type ProjectionRequest = { arm: string; pose: PreviewPose; position: number[] };

export interface SolvedProjection {
  arm: string;
  requestPose: PreviewPose;
  projection: PoseProjection;
  requestedPosition: number[];
}

// SAFETY: A stale target's preview may animate the ghost but cannot authorize a move;
// clearing the target invalidates all outstanding answers.
export function createProjectionController(dependencies: {
  currentRequest: () => ProjectionRequest | null;
  preview: (arm: string, pose: PreviewPose) => Promise<PreviewAnswer>;
  onPending: () => void;
  onCalculating: () => void;
  onClear: () => void;
  onFailure: (message: string) => void;
  onSolution: (solved: SolvedProjection, confirm: boolean) => void;
}) {
  // INVARIANT: Bumped when the target is cleared: an answer from an older epoch is dropped.
  let epoch = 0;
  let hasTarget = false;
  const latestRequest = createLatestOnly<ProjectionRequest>(
    async (request) => {
      const askedInEpoch = epoch;
      const answer = await dependencies.preview(request.arm, request.pose);
      if (askedInEpoch !== epoch) return;
      if (answer.status === "refused") throw new Error(answer.detail);
      const current = dependencies.currentRequest();
      dependencies.onSolution(
        {
          arm: request.arm,
          requestPose: request.pose,
          projection: answer.projection,
          requestedPosition: request.position,
        },
        JSON.stringify(current?.pose) === JSON.stringify(request.pose),
      );
    },
    (error) => {
      dependencies.onFailure(errorMessage(error));
    },
  );
  return {
    targetChanged(): void {
      if (!hasTarget) dependencies.onCalculating();
      hasTarget = true;
      dependencies.onPending();
      const request = dependencies.currentRequest();
      if (request) latestRequest.push(request);
    },
    clear(): void {
      epoch += 1;
      hasTarget = false;
      latestRequest.cancel();
      dependencies.onClear();
    },
    destroy(): void {
      epoch += 1;
      latestRequest.cancel();
    },
  };
}

export function interpretProjection(
  solved: SolvedProjection,
  confirm: boolean,
) {
  const { projection, requestedPosition } = solved;
  const { outcome } = projection;
  const orientation =
    projection.requested?.orientation ??
    (outcome.kind === "unavailable" ? null : outcome.pose.orientation);
  const chosenOrientation = orientation
    ? {
        kind: "grasp" as const,
        approach: orientation.approach,
        jaw_yaw_degrees: orientation.jaw_yaw_degrees,
      }
    : null;
  if (outcome.kind === "unavailable")
    return {
      kind: "unavailable" as const,
      chosenOrientation,
      jawYawRanges: projection.jaw_yaw_ranges,
      reason: outcome.reason,
    };
  const pose = outcome.pose;
  return {
    kind: "available" as const,
    chosenOrientation,
    jawYawRanges: projection.jaw_yaw_ranges,
    joints: pose.joints,
    withinTolerance: outcome.within_tolerance,
    message: outcome.within_tolerance
      ? "Goal reachable within tolerance. The ghost shows the proposed pose."
      : `Preview ready. The ghost shows an approximate pose: ${outcome.reason ?? "The goal was not met within tolerance."}`,
    confirmedPose: confirm
      ? {
          joints: pose.joints,
          position: pose.position,
          quaternion: pose.quaternion,
          requestedPosition,
          requestedQuaternion: projection.requested?.quaternion ?? null,
        }
      : null,
  };
}

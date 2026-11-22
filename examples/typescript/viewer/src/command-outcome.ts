import type { CommandRecord } from "hebbian-rua";
import { formatMotionDetail, radiansToDegrees } from "./shared/measurements.js";

// This presentation threshold is for the manual viewer only; the station's
// result and its settling and safety limits remain unchanged.
const APPROXIMATE_JOINT_ERROR_RADIANS = 0.06;

export function describeCommandOutcome(command: CommandRecord): {
  text: string;
  isError: boolean;
} {
  const jointErrors = command.achieved?.joint_errors_radians;
  const completion = command.completion;
  const approximatelyReached =
    completion?.outcome === "stopped_short" &&
    ["move_to_joints", "move_to_pose"].includes(command.label) &&
    jointErrors != null &&
    jointErrors.length > 0 &&
    jointErrors.every(
      (error) =>
        Number.isFinite(error) &&
        Math.abs(error) <= APPROXIMATE_JOINT_ERROR_RADIANS,
    );
  if (approximatelyReached) {
    const maximumError = Math.max(...jointErrors.map(Math.abs));
    return {
      text: `Last: ${command.label} reached approximately — maximum joint error ${radiansToDegrees(maximumError).toFixed(1)}°`,
      isError: false,
    };
  }
  return {
    text: completion
      ? `Last: ${command.label} ${completion.outcome.replaceAll("_", " ")}${completion.detail ? ` — ${formatMotionDetail(completion.detail)}` : ""}`
      : "",
    isError: ["failed", "faulted", "tripped", "stopped_short"].includes(
      completion?.outcome ?? "",
    ),
  };
}

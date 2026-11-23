import { test } from "vitest";
import {
  describeApproach,
  jawYawReachable,
  moveCommandForPreview,
  movePreviewedGripper,
  syncCurrentPosePreview,
  unreachableYawLines,
} from "../src/preview-move.js";

test("preview move", async () => {
  function assert(condition: unknown, message: string): asserts condition {
    if (!condition) throw new Error(message);
  }
  const openings: number[] = [];
  let finishMove: ((accepted: boolean) => void) | undefined;
  const move = new Promise<boolean>((resolve) => {
    finishMove = resolve;
  });
  const pending = movePreviewedGripper(
    () => move,
    () => true,
    true,
    0.7,
    (opening) => openings.push(opening),
  );
  assert(
    openings.length === 0,
    "Gripper must wait for the arm command to be accepted",
  );
  assert(finishMove, "Move is pending");
  finishMove(true);
  await pending;
  assert(
    Number(openings.length) === 1 && openings[0] === 0.7,
    "Accepted move must apply previewed opening",
  );
  await movePreviewedGripper(
    async () => false,
    () => true,
    true,
    0.1,
    (opening) => openings.push(opening),
  );
  await movePreviewedGripper(
    async () => true,
    () => false,
    true,
    0.2,
    (opening) => openings.push(opening),
  );
  await movePreviewedGripper(
    async () => true,
    () => true,
    false,
    0.3,
    (opening) => openings.push(opening),
  );
  assert(
    Number(openings.length) === 1,
    "Rejected, stopped or gripper-free moves must not command the gripper",
  );

  const previewOpenings: number[] = [];
  const targetPositions: number[][] = [];
  const synced = syncCurrentPosePreview(
    {
      tool: {
        position: [0.1, 0.2, 0.3],
        quaternion: [0, 0, 0, 1],
        orientation: {
          approach: [0, 0, -1],
          degrees_from_down: 0,
          jaw_yaw_degrees: 0,
          jaw_yaw_reference: "world_x",
          goal: null,
        },
        model_points: [],
      },
      gripper_opening: 0.42,
    },
    (opening) => previewOpenings.push(opening),
    (position) => targetPositions.push(position),
  );
  assert(
    synced && previewOpenings[0] === 0.42 && targetPositions[0]?.[0] === 0.1,
    "Current pose must copy measured jaw and tool position",
  );
  assert(
    !syncCurrentPosePreview(
      { tool: null, gripper_opening: null },
      () => {
        throw Error("No pose");
      },
      () => {
        throw Error("No pose");
      },
    ),
    "Missing tool position cannot initialize a target",
  );

  const ranges = [
    {
      from_degrees: 0,
      to_degrees: 20,
      reachable: false,
      reason: "Wrist limit",
    },
    { from_degrees: 20, to_degrees: 340, reachable: true, reason: null },
    {
      from_degrees: 340,
      to_degrees: 360,
      reachable: false,
      reason: "Wrist limit",
    },
  ];
  assert(
    jawYawReachable(ranges, 10) === false &&
      jawYawReachable(ranges, 90) === true &&
      jawYawReachable(ranges, -10) === false &&
      jawYawReachable(ranges, 370) === false,
    "A yaw is judged against the station's ranges, wrapping full turns",
  );
  assert(
    jawYawReachable([], 90) === null,
    "Without ranges the yaw is not judged",
  );
  assert(
    unreachableYawLines(ranges).join("|") ===
      "0°–20°: Wrist limit|340°–360°: Wrist limit",
    "Unreachable ranges are listed with the station's reasons",
  );
  assert(
    describeApproach([0.6, 0, -0.8]) === "custom (0.60, 0.00, -0.80)",
    "Named approaches are presets; other directions are custom",
  );

  const previewedPose = {
    joints: [0.1, 0.2, 0.3, 0.4, 0.5],
    position: [0.25, 0, 0.1],
    quaternion: [0, 0, 0, 1],
    requestedPosition: [0.3, 0, 0.1],
    requestedQuaternion: [0, 1, 0, 0],
  };
  const ikMove = moveCommandForPreview("ik", previewedPose, 0.5);
  assert(
    ikMove?.kind === "move_to_joints" && ikMove.joints[4] === 0.5,
    "An IK arm moves to the ghost's joints",
  );
  const policyMove = moveCommandForPreview("policy", previewedPose, 0.5);
  assert(
    policyMove?.kind === "move_to_pose" &&
      policyMove.position[0] === 0.3 &&
      policyMove.orientation.kind === "quaternion" &&
      policyMove.orientation.quaternion[1] === 1,
    "A policy arm is sent the requested pose, not the rehearsal's end",
  );
  assert(
    moveCommandForPreview(
      "policy",
      { ...previewedPose, requestedQuaternion: null },
      0.5,
    ) === null,
    "Without the station's quaternion nothing is sent",
  );
});

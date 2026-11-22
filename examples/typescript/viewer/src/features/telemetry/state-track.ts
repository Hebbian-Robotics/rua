import type { StationState } from "hebbian-rua";

export interface ArmTrack {
  arm: string;
  basePose: StationState["arms"][number]["base_pose"];

  measuredJoints: (number | null)[][];
  commandedJoints: (number | null)[][];
  destinationJoints: (number[] | null)[];
  destinationLimited: boolean[];
  gripperOpening: (number | null)[];
}

export interface StateTrack {
  times: number[];
  jointNames: string[];
  arms: ArmTrack[];
}

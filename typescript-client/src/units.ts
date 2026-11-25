// Motion unit conversions backed by the shared station contract.

import { robotToolInputUnits } from "./generated/robot-tools.js";

export type LengthUnit = "m" | "mm";
export type AngleUnit = "rad" | "deg";

function modelInputScale(field: "position_mm" | "jaw_yaw_degrees"): number {
  const unit = robotToolInputUnits.find((entry) => entry.model_field === field);
  if (unit === undefined) throw new Error(`Missing motion unit for ${field}`);
  return unit.scale_to_station;
}

const metresPerMillimetre = modelInputScale("position_mm");
const radiansPerDegree = modelInputScale("jaw_yaw_degrees");

/** Convert a world position to the metres expected by station motion commands. */
export function positionMetres(
  position: readonly [number, number, number],
  unit: LengthUnit,
): [number, number, number] {
  const [x, y, z] = position;
  switch (unit) {
    case "m":
      return [x, y, z];
    case "mm":
      return [
        x * metresPerMillimetre,
        y * metresPerMillimetre,
        z * metresPerMillimetre,
      ];
  }
  throw new RangeError(`Invalid length unit: ${unit satisfies never}`);
}

/** Convert an angle or angular rate to radians, retaining its time denominator. */
export function angularRadians(value: number, unit: AngleUnit): number {
  switch (unit) {
    case "rad":
      return value;
    case "deg":
      return value * radiansPerDegree;
  }
  throw new RangeError(`Invalid angle unit: ${unit satisfies never}`);
}

/** Convert an angle or angular rate to degrees, retaining its time denominator. */
export function angularDegrees(value: number, unit: AngleUnit): number {
  switch (unit) {
    case "rad":
      return value * (1 / radiansPerDegree);
    case "deg":
      return value;
  }
  throw new RangeError(`Invalid angle unit: ${unit satisfies never}`);
}

/** Convert a joint target to radians in the supplied joint order. */
export function jointRadians(
  joints: readonly number[],
  unit: AngleUnit,
): number[] {
  return joints.map((angle) => angularRadians(angle, unit));
}

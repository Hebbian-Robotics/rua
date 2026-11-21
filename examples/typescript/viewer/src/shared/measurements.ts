const numericText = String.raw`[+-]?(?:\d+(?:\.\d*)?|\.\d+)(?:[eE][+-]?\d+)?`;
const angularQuantity = new RegExp(
  String.raw`(?<![\w.])(${numericText}) (rad/s|rad)\b`,
  "g",
);
const angleRange = new RegExp(
  String.raw`\[(${numericText}), (\s*${numericText})\] rad\b`,
  "g",
);
const angleList = new RegExp(
  String.raw`\(rad\): \[(${numericText}(?:, ${numericText})*)\]`,
  "g",
);

export function radiansToDegrees(radians: number): number {
  return (radians * 180) / Math.PI;
}

export function millimetersToMeters(millimeters: number): number {
  return millimeters / 1000;
}

export function metersToMillimeters(meters: number): number {
  return meters * 1000;
}

function displayAngle(radians: string): string {
  return String(Number(radiansToDegrees(Number(radians)).toFixed(3)));
}

// CONSTRAINT: The station sends diagnostics as text. Convert the quantities
// where the viewer presents them while retaining the original diagnostic on the wire.
export function formatMotionDetail(detail: string): string {
  return detail
    .replace(
      angleRange,
      (_, lower: string, upper: string) =>
        `[${displayAngle(lower)}, ${displayAngle(upper)}]°`,
    )
    .replace(
      angleList,
      (_, angles: string) =>
        `(°): [${angles.split(", ").map(displayAngle).join(", ")}]`,
    )
    .replace(
      angularQuantity,
      (_, radians: string, unit: string) =>
        `${displayAngle(radians)}${unit === "rad/s" ? "°/s" : "°"}`,
    );
}

export function isJointTargetOutsideLimits(detail: string): boolean {
  return /^Joint \d+ target .* rad is outside \[.*\] rad$/.test(detail);
}

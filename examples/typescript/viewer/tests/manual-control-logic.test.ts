import { expect, test } from "vitest";
import {
  canTakeManualControl,
  manualMoveBlockedReason,
  previewMatchesSelection,
} from "../src/features/control/manual-control-logic.js";

const ready = {
  ownershipReason: null,
  armMode: "holding" as const,
  hasTarget: true,
  hasConfirmedPreview: true,
  projectionValidity: true,
};

test("manual control requires an enabled arm and can take over while it moves", () => {
  expect(canTakeManualControl("holding")).toBe(true);
  expect(canTakeManualControl("moving")).toBe(true);
  for (const mode of [
    null,
    "disabled",
    "reconnected",
    "faulted",
    "disconnected",
  ] as const)
    expect(canTakeManualControl(mode)).toBe(false);
});

test("a move requires control, a holding arm, and a current validated preview", () => {
  expect(manualMoveBlockedReason(ready)).toBeNull();
  for (const change of [
    { ownershipReason: "Control lease held elsewhere." },
    { armMode: "disabled" as const },
    { hasTarget: false },
    { hasConfirmedPreview: false },
    { projectionValidity: null },
    { projectionValidity: false },
  ])
    expect(manualMoveBlockedReason({ ...ready, ...change })).not.toBeNull();
});

test("preview safety checks distinguish pending evaluation from a blocked pose", () => {
  expect(manualMoveBlockedReason({ ...ready, projectionValidity: null })).toBe(
    "Checking preview safety.",
  );
  expect(manualMoveBlockedReason({ ...ready, projectionValidity: false })).toBe(
    "Preview blocked by the table surface or model unavailable.",
  );
});

test("a confirmed preview cannot be reused for a different arm, target, or orientation", () => {
  const base = {
    confirmed: { arm: "left", requestPose: { position: [0.2, 0, 0.1] } },
    arm: "left",
    projectionArm: "left",
    target: [0.2, 0, 0.1],
    orientation: null,
  };
  expect(previewMatchesSelection(base)).toBe(true);
  expect(previewMatchesSelection({ ...base, arm: "right" })).toBe(false);
  expect(previewMatchesSelection({ ...base, projectionArm: "right" })).toBe(
    false,
  );
  expect(previewMatchesSelection({ ...base, target: [0.3, 0, 0.1] })).toBe(
    false,
  );
  expect(
    previewMatchesSelection({
      ...base,
      orientation: { kind: "grasp", approach: "down", jaw_yaw_degrees: 0 },
    }),
  ).toBe(false);
});

import { expect, test } from "vitest";
import { controlModeLockReason } from "../src/features/control/control-mode.js";

test("control mode can change once every arm is confirmed off", () => {
  expect(
    controlModeLockReason([{ mode: "disabled" }], false, false),
  ).toBeNull();
  expect(
    controlModeLockReason(
      [{ mode: "disabled" }, { mode: "reconnected" }],
      false,
      false,
    ),
  ).toBeNull();
  expect(controlModeLockReason([{ mode: "holding" }], false, false)).toBe(
    "Disable the arm to switch modes.",
  );
  expect(
    controlModeLockReason(
      [{ mode: "disabled" }, { mode: "moving" }],
      false,
      false,
    ),
  ).toBe("Disable all arms to switch modes.");
  expect(controlModeLockReason([], false, false)).toBe(
    "Waiting for arm status.",
  );
});

test("mode switching stays locked while an arm is powering up or an agent run is active", () => {
  expect(controlModeLockReason([{ mode: "disabled" }], false, true)).toBe(
    "Wait for the arm power change to finish.",
  );
  expect(controlModeLockReason([{ mode: "disabled" }], true, false)).toBe(
    "End the agent run before switching modes.",
  );
});

import { expect, test } from "vitest";
import { formatMotionDetail } from "../src/shared/measurements.js";

test("a joint target just outside its limit remains distinguishable in degrees", () => {
  expect(
    formatMotionDetail(
      "Joint 1 target -1.7454287090887923 rad is outside [-1.74533, 1.74533] rad",
    ),
  ).toBe("Joint 1 target -100.006° is outside [-100, 100]°");
});

test("motion diagnostics display angular speeds and multiple joint errors in degrees", () => {
  expect(
    formatMotionDetail(
      "Feedback tracking error was 0.1 rad; commanded 0.2 rad/s, measured -0.1 rad/s",
    ),
  ).toBe(
    "Feedback tracking error was 5.73°; commanded 11.459°/s, measured -5.73°/s",
  );
  expect(
    formatMotionDetail(
      "Stopped short; final joint errors (rad): [0.051, 0.000]",
    ),
  ).toBe("Stopped short; final joint errors (°): [2.922, 0]");
});

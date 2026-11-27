import { expect, test } from "vitest";
import {
  readViewerPreferences,
  writeViewerPreferences,
} from "../src/features/settings/preferences.js";

function storage(
  initial: Record<string, string> = {},
): Pick<Storage, "getItem" | "setItem"> {
  const entries = new Map(Object.entries(initial));
  return {
    getItem: (key) => entries.get(key) ?? null,
    setItem: (key, value) => {
      entries.set(key, value);
    },
  };
}

test("viewer preferences follow the system theme and hide commanded joints by default", () => {
  expect(readViewerPreferences(storage())).toEqual({
    version: 1,
    motionLimits: {},
    theme: "system",
    showTargetJoints: true,
    showCommandedJoints: false,
    showCameraLabels: true,
    agentModelSelection: null,
    recordWithControl: true,
  });
});

test("saved preferences are shared and invalid or missing values get safe defaults", () => {
  const preferencesStorage = storage();
  const preferences = {
    ...readViewerPreferences(preferencesStorage),
    showTargetJoints: false,
    showCommandedJoints: true,
    agentModelSelection: {
      provider: "gemini" as const,
      model: "gemini-4-hydrogen",
    },
    recordWithControl: true,
    theme: "light" as const,
    motionLimits: {
      max_joint_speed: 0.55,
      max_joint_acceleration: 2,
      max_joint_jerk: 40,
    },
  };
  writeViewerPreferences(preferencesStorage, preferences);
  expect(readViewerPreferences(preferencesStorage)).toEqual(preferences);
  const invalidStorage = storage({
    "hebbian-viewer-preferences":
      '{"version":1,"showTargetJoints":"yes","showCommandedJoints":"yes"}',
  });
  expect(readViewerPreferences(invalidStorage)).toMatchObject({
    theme: "system",
    showTargetJoints: true,
    showCommandedJoints: false,
    agentModelSelection: null,
    recordWithControl: true,
  });
});

test("unavailable or corrupt storage does not prevent opening the viewer", () => {
  const unavailable = {
    getItem: () => {
      throw new Error("blocked");
    },
    setItem: () => {
      throw new Error("blocked");
    },
  };
  const preferences = readViewerPreferences(unavailable);
  expect(preferences.showTargetJoints).toBe(true);
  expect(() => writeViewerPreferences(unavailable, preferences)).not.toThrow();
  expect(
    readViewerPreferences(storage({ "hebbian-viewer-preferences": "{" })).theme,
  ).toBe("system");
});

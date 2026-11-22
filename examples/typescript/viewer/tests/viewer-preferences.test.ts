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
    theme: "system",
    showTargetJoints: true,
    showCommandedJoints: false,
    showCameraLabels: true,
    agentServerUrl: "",
    recordWithControl: false,
  });
});

test("saved preferences are shared and invalid or missing values get safe defaults", () => {
  const preferencesStorage = storage();
  const preferences = {
    ...readViewerPreferences(preferencesStorage),
    showTargetJoints: false,
    showCommandedJoints: true,
    recordWithControl: true,
    theme: "light" as const,
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
    recordWithControl: false,
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

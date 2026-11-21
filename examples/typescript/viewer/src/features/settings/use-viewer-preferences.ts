import { useLayoutEffect, useState, useSyncExternalStore } from "react";
import {
  defaultViewerPreferences,
  readViewerPreferences,
  type ViewerColorScheme,
  type ViewerPreferences,
  writeViewerPreferences,
} from "./preferences.js";

function browserPreferences(): ViewerPreferences {
  try {
    return readViewerPreferences(window.localStorage);
  } catch {
    return defaultViewerPreferences();
  }
}

const darkModeQuery = "(prefers-color-scheme: dark)";

function subscribeToSystemTheme(onChange: () => void): () => void {
  const systemTheme = window.matchMedia(darkModeQuery);
  systemTheme.addEventListener("change", onChange);
  return () => systemTheme.removeEventListener("change", onChange);
}

function readSystemColorScheme(): ViewerColorScheme {
  return window.matchMedia(darkModeQuery).matches ? "dark" : "light";
}

export function useViewerPreferences() {
  const [preferences, setPreferences] = useState(browserPreferences);
  const systemColorScheme = useSyncExternalStore(
    subscribeToSystemTheme,
    readSystemColorScheme,
  );
  const colorScheme =
    preferences.theme === "system" ? systemColorScheme : preferences.theme;
  useLayoutEffect(() => {
    document.documentElement.setAttribute("data-theme", colorScheme);
  }, [colorScheme]);
  function updatePreferences(next: ViewerPreferences): void {
    setPreferences(next);
    try {
      writeViewerPreferences(window.localStorage, next);
    } catch {
      /* Storage access may itself be blocked. */
    }
  }
  return { preferences, updatePreferences, colorScheme };
}

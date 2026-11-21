import type { ArmState } from "@hebbian/robot-use-client";
import { BotIcon, CameraIcon, SunIcon } from "lucide-react";
import { useEffect, useState } from "react";
import { Card } from "@/components/ui/card.js";
import { Input } from "@/components/ui/input.js";
import {
  NativeSelect,
  NativeSelectOption,
} from "@/components/ui/native-select.js";
import { Switch } from "@/components/ui/switch.js";
import { resolveAgentUrl } from "../../agent-api.js";
import {
  readViewerPreferences,
  type ViewerPreferences,
  writeViewerPreferences,
} from "./preferences.js";

function browserPreferences(): ViewerPreferences {
  try {
    return readViewerPreferences(window.localStorage);
  } catch {
    return {
      version: 1,
      theme: "system",
      showTargetJoints: true,
      showCommandedJoints: true,
      showCameraLabels: true,
      agentServerUrl: "",
    };
  }
}

export function useViewerPreferences(): [
  ViewerPreferences,
  (next: ViewerPreferences) => void,
] {
  const [preferences, setPreferences] = useState(browserPreferences);
  useEffect(() => {
    const systemTheme = window.matchMedia("(prefers-color-scheme: dark)");
    function applyTheme(): void {
      document.documentElement.setAttribute(
        "data-theme",
        preferences.theme === "system"
          ? systemTheme.matches
            ? "dark"
            : "light"
          : preferences.theme,
      );
    }
    applyTheme();
    systemTheme.addEventListener("change", applyTheme);
    return () => systemTheme.removeEventListener("change", applyTheme);
  }, [preferences.theme]);
  function update(next: ViewerPreferences): void {
    setPreferences(next);
    try {
      writeViewerPreferences(window.localStorage, next);
    } catch {
      /* Storage access may itself be blocked. */
    }
  }
  return [preferences, update];
}

export function SettingsPage({
  preferences,
  update,
  arms,
}: {
  preferences: ViewerPreferences;
  update: (next: ViewerPreferences) => void;
  arms: ArmState[] | null;
}) {
  return (
    <main className="settings-page">
      <header>
        <h1>Settings</h1>
        <p>
          Appearance and visualization preferences are stored in this browser.
        </p>
      </header>
      <Card className="my-4 gap-2 p-5">
        <h2>
          <SunIcon aria-hidden="true" /> Appearance
        </h2>
        <label htmlFor="appearance-theme">
          Theme
          <NativeSelect
            id="appearance-theme"
            value={preferences.theme}
            onChange={(event) => {
              const theme = event.currentTarget.value;
              if (theme === "light" || theme === "dark" || theme === "system")
                update({ ...preferences, theme });
            }}
          >
            <NativeSelectOption value="system">System</NativeSelectOption>
            <NativeSelectOption value="light">Light</NativeSelectOption>
            <NativeSelectOption value="dark">Dark</NativeSelectOption>
          </NativeSelect>
        </label>
      </Card>
      <Card className="my-4 gap-2 p-5">
        <h2>
          <BotIcon aria-hidden="true" /> Arm controller
        </h2>
        {arms?.length ? (
          arms.map((arm) => (
            <p key={arm.arm}>
              {arm.arm}: {arm.controller === "ik" ? "IK" : "Policy"}
            </p>
          ))
        ) : (
          <p>Connect to a station to see its arm controllers.</p>
        )}
        <p>Controllers are configured in the station profile.</p>
      </Card>
      <Card className="my-4 gap-2 p-5">
        <h2>
          <BotIcon aria-hidden="true" /> Robot visualization
        </h2>
        <p>Show overlays on the live robot and during recording playback.</p>
        <label className="setting-switch" htmlFor="target-joints">
          <span>Target joints</span>
          <Switch
            id="target-joints"
            checked={preferences.showTargetJoints}
            onCheckedChange={(checked) =>
              update({ ...preferences, showTargetJoints: checked })
            }
          />
        </label>
        <label className="setting-switch" htmlFor="commanded-joints">
          <span>Commanded joints</span>
          <Switch
            id="commanded-joints"
            checked={preferences.showCommandedJoints}
            onCheckedChange={(checked) =>
              update({ ...preferences, showCommandedJoints: checked })
            }
          />
        </label>
      </Card>
      <Card className="my-4 gap-2 p-5">
        <h2>
          <BotIcon aria-hidden="true" /> Agent server
        </h2>
        <p>
          The optional server that starts agent loops. Leave empty for this host
          on port 8793.
        </p>
        <label htmlFor="agent-server-address">
          <span>Address</span>
          <Input
            id="agent-server-address"
            type="url"
            value={preferences.agentServerUrl}
            placeholder={resolveAgentUrl("")}
            onChange={(event) =>
              update({
                ...preferences,
                agentServerUrl: event.currentTarget.value,
              })
            }
          />
        </label>
      </Card>
      <Card className="my-4 gap-2 p-5">
        <h2>
          <CameraIcon aria-hidden="true" /> Cameras
        </h2>
        <label className="setting-switch" htmlFor="camera-labels">
          <span>Show camera names</span>
          <Switch
            id="camera-labels"
            checked={preferences.showCameraLabels}
            onCheckedChange={(checked) =>
              update({ ...preferences, showCameraLabels: checked })
            }
          />
        </label>
      </Card>
    </main>
  );
}

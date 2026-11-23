import type { ArmState } from "hebbian-rua";
import { BotIcon, SunIcon } from "lucide-react";
import { useEffect } from "react";
import { Card } from "@/components/ui/card.js";
import {
  NativeSelect,
  NativeSelectOption,
} from "@/components/ui/native-select.js";
import { Switch } from "@/components/ui/switch.js";
import type { useAgentRun } from "../agent/use-agent-run.js";
import type { ViewerPreferences } from "./preferences.js";

export function SettingsPage({
  preferences,
  update,
  arms,
  agent,
  section,
}: {
  preferences: ViewerPreferences;
  update: (next: ViewerPreferences) => void;
  arms: ArmState[] | null;
  agent: ReturnType<typeof useAgentRun>;
  section: string | undefined;
}) {
  const { status, connectionError } = agent;
  useEffect(() => {
    const heading = document.getElementById(
      section === "agent" ? "settings-agent" : "settings-heading",
    );
    heading?.focus();
  }, [section]);
  return (
    <main className="settings-page">
      <header>
        <h1 id="settings-heading" tabIndex={-1}>
          Settings
        </h1>
        <p>Preferences are saved in this browser.</p>
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
        <h2>Robot view and camera labels</h2>
        <p>
          These overlays appear in the 3D scene in Workspace and recording
          playback.
        </p>
        <label className="setting-switch" htmlFor="target-joints">
          <span>
            Destination pose{" "}
            <span className="font-normal text-muted-foreground">(green)</span>
          </span>
          <Switch
            id="target-joints"
            checked={preferences.showTargetJoints}
            onCheckedChange={(checked) =>
              update({ ...preferences, showTargetJoints: checked })
            }
          />
        </label>
        <label className="setting-switch" htmlFor="commanded-joints">
          <span>
            Commanded pose{" "}
            <span className="font-normal text-muted-foreground">(purple)</span>
          </span>
          <Switch
            id="commanded-joints"
            checked={preferences.showCommandedJoints}
            onCheckedChange={(checked) =>
              update({ ...preferences, showCommandedJoints: checked })
            }
          />
        </label>
        <label className="setting-switch" htmlFor="camera-labels">
          <span>Show names on live camera feeds</span>
          <Switch
            id="camera-labels"
            checked={preferences.showCameraLabels}
            onCheckedChange={(checked) =>
              update({ ...preferences, showCameraLabels: checked })
            }
          />
        </label>
      </Card>
      <Card className="my-4 gap-2 p-5">
        <h2 id="settings-agent" tabIndex={-1}>
          <BotIcon aria-hidden="true" /> Agent connection
        </h2>
        <p role="status">
          {status === "unreachable"
            ? connectionError
            : status === "disabled"
              ? "Disabled in manual-only mode"
              : status}
        </p>
      </Card>
    </main>
  );
}

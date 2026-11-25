import type { ArmState } from "hebbian-rua";
import { BotIcon, SunIcon } from "lucide-react";
import { useEffect, useState } from "react";
import { Button } from "@/components/ui/button.js";
import { Card } from "@/components/ui/card.js";
import { Input } from "@/components/ui/input.js";
import {
  NativeSelect,
  NativeSelectOption,
} from "@/components/ui/native-select.js";
import { Switch } from "@/components/ui/switch.js";
import type { MotionLimits } from "../../../shared/run-api.js";
import type { useAgentRun } from "../agent/use-agent-run.js";
import type { ViewerPreferences } from "./preferences.js";

const motionFields = [
  ["max_joint_speed", "Joint speed (rad/s)"],
  ["max_joint_acceleration", "Joint acceleration (rad/s²)"],
  ["max_joint_jerk", "Joint jerk (rad/s³)"],
] as const;

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
  const [keyError, setKeyError] = useState("");
  const { status, connectionError } = agent;
  const [motionLimitsSaved, setMotionLimitsSaved] = useState(false);
  useEffect(() => {
    const heading = document.getElementById(
      section?.startsWith("key-")
        ? section
        : section === "agent"
          ? "settings-agent"
          : "settings-heading",
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
        <details>
          <summary className="cursor-pointer font-medium">
            Advanced motion limits
          </summary>
          <form
            aria-label="Motion limits"
            aria-describedby="motion-limits-help"
            className="mt-3 grid gap-3"
            onSubmit={(event) => {
              event.preventDefault();
              const form = event.currentTarget;
              const motionLimits: MotionLimits = {};
              for (const [name] of motionFields) {
                const field = form.elements.namedItem(name);
                if (!(field instanceof HTMLInputElement) || field.value === "")
                  continue;
                const value = field.valueAsNumber;
                if (!Number.isFinite(value) || value <= 0) {
                  field.setCustomValidity(
                    "Enter a number greater than zero, or leave this blank.",
                  );
                  field.reportValidity();
                  return;
                }
                motionLimits[name] = value;
              }
              update({ ...preferences, motionLimits });
              setMotionLimitsSaved(true);
            }}
          >
            <p id="motion-limits-help">
              Applies to planned IK moves from the viewer and its agent runs.
              The speed cap also applies when dragging in Follow mode. Leave a
              field blank to use the arm profile.
            </p>
            <fieldset className="grid gap-2">
              <legend className="sr-only">Per-move caps</legend>
              {motionFields.map(([name, label]) => (
                <label key={name} className="grid gap-1" htmlFor={name}>
                  {label}
                  <Input
                    id={name}
                    name={name}
                    type="number"
                    step="any"
                    min="0"
                    placeholder="Arm profile"
                    defaultValue={preferences.motionLimits[name] ?? ""}
                    onInput={(event) => {
                      event.currentTarget.setCustomValidity("");
                      setMotionLimitsSaved(false);
                    }}
                  />
                </label>
              ))}
            </fieldset>
            <Button type="submit" variant="outline" className="w-fit">
              Save motion limits
            </Button>
            <p role="status">
              {motionLimitsSaved ? "Motion limits saved." : ""}
            </p>
          </form>
        </details>
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
          {status === "unreachable" ? connectionError : status}
        </p>
        <h2>Model API keys</h2>
        {agent.providersError && <p role="alert">{agent.providersError}</p>}
        {agent.providers?.map(({ provider, label, available }) => {
          const saved = Boolean(agent.browserKey(provider));
          const id = `key-${provider}`;
          if (available)
            return (
              <p key={provider} id={id} tabIndex={-1}>
                {label} · Set in viewer environment
              </p>
            );
          function save(value: string) {
            try {
              agent.saveKey(provider, value);
              setKeyError("");
              return true;
            } catch {
              setKeyError("Browser storage is unavailable.");
              return false;
            }
          }
          return (
            <form
              key={provider}
              aria-label={`${label} key`}
              className="grid gap-2"
              method="post"
              onSubmit={(event) => {
                event.preventDefault();
                const form = event.currentTarget;
                const value = new FormData(form).get("apiKey");
                if (
                  typeof value === "string" &&
                  value.trim() &&
                  save(value.trim())
                )
                  form.reset();
              }}
            >
              <label htmlFor={id}>{label} API key</label>
              <Input
                type="password"
                id={id}
                name="apiKey"
                required
                pattern=".*\S.*"
                autoCapitalize="none"
                spellCheck={false}
              />
              <p role="status">
                {saved ? "Saved in this browser" : "No key set"}
              </p>
              <div>
                <Button type="submit">
                  {saved ? "Replace key" : "Save in this browser"}
                </Button>
                {saved && (
                  <Button
                    variant="outline"
                    type="button"
                    onClick={() => save("")}
                  >
                    Remove
                  </Button>
                )}
              </div>
            </form>
          );
        })}
        {keyError && <p role="alert">{keyError}</p>}
        <a href="#workspace-control">Back to Agent Run</a>
      </Card>
    </main>
  );
}

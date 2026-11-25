import type { LiveRun, StationState } from "hebbian-rua";
import { commandPlan, commandWasLimited } from "hebbian-rua";
import { BotIcon, HandIcon, TriangleAlertIcon } from "lucide-react";
import {
  type ReactNode,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { Alert } from "@/components/ui/alert.js";
import { Button } from "@/components/ui/button.js";
import { Card } from "@/components/ui/card.js";
import { agentApi } from "../agent-api.js";
import { errorMessage } from "../errors.js";
import { AgentControl } from "../features/agent/agent-control.js";
import { useAgentRun } from "../features/agent/use-agent-run.js";
import {
  type CameraRoomState,
  CameraView,
} from "../features/cameras/live-cameras.js";
import { PhoneCameraSetup } from "../features/cameras/phone-camera-setup.js";
import { useMediaStatus } from "../features/cameras/use-media-status.js";
import { ArmControl } from "../features/control/arm-control.js";
import { controlModeLockReason } from "../features/control/control-mode.js";
import { ControllerRecovery } from "../features/control/controller-recovery.js";
import { ManualControl } from "../features/control/manual-control.js";
import {
  RobotScene,
  type SceneProjection,
} from "../features/robot-scene/robot-scene.js";
import { RunPanels } from "../features/run/run-panels.js";
import { SettingsPage } from "../features/settings/settings-page.js";
import type { useViewerPreferences } from "../features/settings/use-viewer-preferences.js";
import { LiveTelemetry } from "../features/telemetry/joint-charts.js";
import { armControl, manualLease } from "../manual-lease.js";
import { isRunActive, type Run, type RunRequest } from "../run-logic.js";
import { formatMotionDetail } from "../shared/measurements.js";
import {
  type ViewerNavigationLink,
  ViewerShell,
} from "../shared/navigation.js";
import { useStationState } from "../use-station-state.js";
import { stationConnection } from "../viewer-connection.js";
import "./ui.css";
import "./workspace.css";

type Position = [number, number, number];
const CONTROL_MODES = [
  { value: "agent", label: "Agent", Icon: BotIcon },
  { value: "manual", label: "Manual", Icon: HandIcon },
] as const;
type ControlMode = (typeof CONTROL_MODES)[number]["value"];
const CONTROL_MODE_KEY = "hebbian-viewer-control-mode:v1";
const EMPTY_SPANS: LiveRun["spans"] = [];
const noSeek = (): void => {};

function RecoveryControls({ state }: { state: StationState | null }) {
  const [messages, setMessages] = useState<Record<string, string>>({});
  const [pending, setPending] = useState<string | null>(null);
  async function recover(armName: string): Promise<void> {
    const arm = state?.arms.find((candidate) => candidate.arm === armName);
    if (arm?.mode !== "faulted") return;
    const fresh =
      arm.sampled_unix_nano !== null &&
      Date.now() / 1000 - Number(arm.sampled_unix_nano) / 1e9 < 1;
    if (!fresh || pending !== null) return;
    setMessages((previous) => ({ ...previous, [armName]: "" }));
    setPending(armName);
    try {
      const answer = await armControl(armName).clearFault();
      if (answer.status === "refused") throw new Error(answer.detail);
    } catch (error: unknown) {
      setMessages((previous) => ({
        ...previous,
        [armName]: `Could not clear fault: ${errorMessage(error)}`,
      }));
    } finally {
      setPending(null);
    }
  }
  return (
    <div className="recovery">
      {state?.arms.map((arm) => {
        const fresh =
          arm.sampled_unix_nano !== null &&
          Date.now() / 1000 - Number(arm.sampled_unix_nano) / 1e9 < 1;
        const issue = !fresh
          ? "Motor feedback is stale. Check controller power and USB."
          : arm.fault || arm.active_trip;
        const error = messages[arm.arm];
        return (
          (issue || error || arm.mode === "faulted") && (
            <Alert
              key={arm.arm}
              role="status"
              variant="destructive"
              className="flex flex-wrap items-center gap-3"
            >
              {(issue || arm.mode === "faulted") && (
                <p className="m-0">
                  {arm.arm}: {formatMotionDetail(issue || "Arm faulted.")}
                </p>
              )}
              {error && <p className="m-0">{formatMotionDetail(error)}</p>}
              {arm.mode === "faulted" && (
                <Button
                  variant="outline"
                  disabled={pending !== null || !fresh}
                  onClick={() => void recover(arm.arm)}
                >
                  {pending === arm.arm
                    ? "Checking…"
                    : `Clear fault: ${arm.arm}`}
                </Button>
              )}
            </Alert>
          )
        );
      })}
    </div>
  );
}

type WorkspaceAppProps = {
  manualOnly: boolean;
  viewerPreferences: ReturnType<typeof useViewerPreferences>;
  cameras: CameraRoomState;
  navigationLinks?: readonly ViewerNavigationLink[];
  statusIndicator?: ReactNode;
  renderAdditionalControls?: (run: Run | null) => ReactNode;
  beforeAgentStart?: () => Promise<void>;
  onManualControlStarted?: () => Promise<void>;
};

export function WorkspaceApp({
  manualOnly,
  viewerPreferences,
  cameras,
  navigationLinks,
  statusIndicator,
  renderAdditionalControls,
  beforeAgentStart,
  onManualControlStarted,
}: WorkspaceAppProps) {
  const [route, setRoute] = useState(() => location.hash);
  const [routePage, settingsSection] = route.split("/");
  const page = routePage === "#settings" ? "settings" : "workspace";
  const [preferredControlMode, setControlMode] = useState<ControlMode>(() => {
    try {
      return sessionStorage.getItem(CONTROL_MODE_KEY) === "manual"
        ? "manual"
        : "agent";
    } catch {
      return "agent";
    }
  });
  const controlMode = manualOnly ? "manual" : preferredControlMode;
  const [selectedArm, setSelectedArm] = useState("");
  const [telemetryOpen, setTelemetryOpen] = useState(false);
  const { stationState, connected } = useStationState();
  const [liveRun, setLiveRun] = useState<LiveRun | null>(null);
  const [currentTime, setCurrentTime] = useState(() => Date.now() / 1000);
  const [target, setTarget] = useState<Position | null>(null);
  const [projection, setProjectionState] = useState<SceneProjection | null>(
    null,
  );
  const [projectionEvaluation, setProjectionEvaluation] = useState<{
    projection: SceneProjection;
    valid: boolean;
  } | null>(null);
  const [dragging, setDragging] = useState(false);
  const [runError, setRunError] = useState("");
  const globalStopHandler = useRef<(() => void) | null>(null);
  const [startingRun, setStartingRun] = useState(false);
  const [changingArmPower, setChangingArmPower] = useState(false);
  const [stoppingRun, setStoppingRun] = useState(false);
  const { preferences, updatePreferences, colorScheme } = viewerPreferences;
  const agentUrl = globalThis.location.origin;
  const agent = useAgentRun(agentUrl, !manualOnly);
  const { run, setRun } = agent;
  const runActive = isRunActive(run);
  const controlModeLocked = controlModeLockReason(
    connected ? (stationState?.arms ?? []) : [],
    startingRun || runActive,
    changingArmPower,
  );
  const canSwitchMode = controlModeLocked === null;
  useEffect(() => {
    if (canSwitchMode) void manualLease.returnLease();
  }, [canSwitchMode]);
  const armNames = stationState?.arms.map((arm) => arm.arm) ?? [];
  const armName = armNames.includes(selectedArm)
    ? selectedArm
    : (armNames[0] ?? "");
  const stopFollowing = useCallback(() => globalStopHandler.current?.(), []);
  const cameraNames = useMemo(
    () => cameras.feeds.map((feed) => feed.name),
    [cameras.feeds],
  );
  const mediaStatus = useMediaStatus();
  const setProjection = useCallback((next: SceneProjection | null) => {
    setProjectionState(next);
  }, []);
  const onProjectionValidityChange = useCallback(
    (valid: boolean) => {
      if (!projection) return;
      setProjectionEvaluation((previous) =>
        previous?.projection === projection && previous.valid === valid
          ? previous
          : { projection, valid },
      );
    },
    [projection],
  );
  useEffect(
    () =>
      manualLease.onChange(() => {
        if (!manualLease.holdsLease()) {
          setTarget(null);
          setProjection(null);
        }
      }),
    [setProjection],
  );
  useEffect(() => {
    const unsubscribeRun = stationConnection.onRun(setLiveRun);
    stationConnection.start();
    const interval = setInterval(() => setCurrentTime(Date.now() / 1000), 1000);
    const onHashChange = () => {
      const nextRoute = location.hash;
      if (nextRoute.split("/")[0] === "#settings") stopFollowing();
      setRoute(nextRoute);
    };
    window.addEventListener("hashchange", onHashChange);
    return () => {
      unsubscribeRun();
      clearInterval(interval);
      window.removeEventListener("hashchange", onHashChange);
    };
  }, [stopFollowing]);
  useEffect(() => {
    if (!route.startsWith("#workspace")) return;
    const fragmentTarget = document.getElementById(route.slice(1));
    if (
      fragmentTarget?.classList.contains("workspace-jump-target") &&
      document.activeElement !== fragmentTarget
    ) {
      fragmentTarget.scrollIntoView();
      fragmentTarget.focus({ preventScroll: true });
    }
  }, [route]);
  useEffect(() => {
    if (!connected) {
      setTarget(null);
      setProjection(null);
    }
  }, [connected, setProjection]);

  function selectControlMode(value: unknown): void {
    if (value !== "agent" && value !== "manual") return;
    if (
      (manualOnly && value === "agent") ||
      value === controlMode ||
      controlModeLockReason(
        stationConnection.isConnected()
          ? (stationConnection.getState()?.arms ?? [])
          : [],
        startingRun || runActive,
        changingArmPower,
      )
    )
      return;

    if (value === "agent") {
      stopFollowing();
      void manualLease.returnLease();
    }
    setControlMode(value);
    try {
      sessionStorage.setItem(CONTROL_MODE_KEY, value);
    } catch {
      // WHY: The selected mode remains usable when browser storage is unavailable.
    }
  }

  async function startRun(request: RunRequest): Promise<void> {
    if (manualOnly || startingRun || runActive) return;
    setStartingRun(true);
    setRunError("");
    try {
      stopFollowing();
      await manualLease.returnLease();
      if (beforeAgentStart) await beforeAgentStart();
      const controller = stationState?.arms.find(
        ({ arm }) => arm === request.arm,
      )?.controller;
      setRun(
        await agentApi.startRun(agentUrl, {
          ...request,
          ...(controller === "ik"
            ? { motionLimits: preferences.motionLimits }
            : {}),
        }),
      );
    } catch (error: unknown) {
      setRunError(`Could not start the run: ${errorMessage(error)}`);
    } finally {
      setStartingRun(false);
    }
  }

  async function stopRun(): Promise<void> {
    if (!run || stoppingRun) return;
    setStoppingRun(true);
    setRunError("");
    try {
      globalStopHandler.current?.();
      void manualLease.returnLease();
      // WHY: The agent server only signals its loop to exit; its lease ends with
      // the loop's connection, which stops the arms.
      if (isRunActive(run)) setRun(await agentApi.stopRun(agentUrl, run.id));
    } catch (error: unknown) {
      setRunError(`Could not stop the run: ${errorMessage(error)}`);
    } finally {
      setStoppingRun(false);
    }
  }
  return (
    <ViewerShell
      current={page}
      onStop={stopFollowing}
      navigationLinks={navigationLinks}
      statusIndicator={statusIndicator}
    >
      <ControllerRecovery />
      <RecoveryControls state={connected ? stationState : null} />
      <main hidden={page !== "workspace"} className="workspace-page">
        <h1 id="workspace" className="workspace-jump-target" tabIndex={-1}>
          Workspace
        </h1>
        <nav
          className="workspace-section-links"
          aria-label="Workspace sections"
        >
          <a href="#workspace-cameras">Cameras</a>
          <a href="#workspace-control">Control</a>
          <a href="#workspace-telemetry">Joint telemetry</a>
          <a href="#workspace-activity">Run activity</a>
        </nav>
        <div className="workspace-layout">
          <div className="workspace-visuals">
            <section
              className="grid min-w-0 gap-3"
              aria-labelledby="workspace-cameras"
            >
              <h2
                id="workspace-cameras"
                className="workspace-jump-target"
                tabIndex={-1}
              >
                Cameras
              </h2>
              <p className="workspace-camera-status" role="status">
                {cameras.status}
                {mediaStatus && <span className="block">{mediaStatus}</span>}
              </p>
              <details className="camera-setup">
                <summary>Camera setup</summary>
                <div className="grid gap-3">
                  <div className="grid gap-1">
                    <h3>Local camera</h3>
                    <p className="m-0 text-sm text-muted-foreground">
                      Plug the wrist camera into the station laptop. The station
                      detects the arm’s registered wrist camera automatically.
                      To use a different camera, run{" "}
                      <code>rua camera-devices</code> on the station and pass{" "}
                      <code>--camera wrist=DEVICE</code> at startup.
                    </p>
                  </div>
                  {!cameras.localCamerasOnly && (
                    <div className="grid gap-2 border-t border-border pt-3">
                      <h3>Remote camera</h3>
                      <p className="m-0 text-sm text-muted-foreground">
                        Create a LiveKit invite for a phone, tablet, or laptop
                        webcam to stream from another angle.
                      </p>
                      <PhoneCameraSetup />
                    </div>
                  )}
                </div>
              </details>
              {cameras.localCamerasOnly && (
                <Alert role="status" variant="destructive">
                  <TriangleAlertIcon aria-hidden="true" />
                  <p className="m-0">
                    <strong>Local camera fallback.</strong> The station was
                    started with <code>--local-only-no-remote-cameras</code>,
                    which is only for when LiveKit is unavailable. Prefer
                    LiveKit whenever possible: set <code>LIVEKIT_URL</code>,{" "}
                    <code>LIVEKIT_API_KEY</code>, and{" "}
                    <code>LIVEKIT_API_SECRET</code> on the station for LiveKit
                    Cloud or a local <code>livekit-server</code>, then restart
                    it without that flag.
                  </p>
                </Alert>
              )}
              {cameras.feeds.length > 0 && (
                <div className="camera-grid">
                  {cameras.feeds.map((feed) => (
                    <CameraView
                      key={feed.name}
                      feed={feed}
                      showLabel={preferences.showCameraLabels}
                    />
                  ))}
                </div>
              )}
            </section>
            <RobotScene
              colorScheme={colorScheme}
              jointNames={stationState?.joint_names ?? []}
              arms={
                stationState?.arms.map((arm) => ({
                  arm: arm.arm,
                  basePose: arm.base_pose,
                  measuredJoints: arm.measured_joints,
                  commandedJoints: arm.commanded_joints,
                  destinationJoints:
                    commandPlan(arm.active_command)?.destination_joints ?? null,
                  destinationLimited: commandWasLimited(arm.active_command),
                  gripperOpening: arm.gripper_opening,
                })) ?? []
              }
              showTarget={preferences.showTargetJoints}
              showCommanded={preferences.showCommandedJoints}
              target={target}
              projection={projection}
              surfaceGuard={stationState?.surface_guard ?? null}
              onTargetChange={setTarget}
              onTargetDrag={setDragging}
              onProjectionValidityChange={onProjectionValidityChange}
            />
          </div>
          <Card
            role="complementary"
            aria-label="Control"
            id="workspace-control"
            tabIndex={-1}
            className="workspace-jump-target control-panel gap-0 py-0"
          >
            <div className="grid gap-2 p-4">
              <fieldset
                className="control-mode-picker"
                aria-describedby="control-mode-hint"
                disabled={controlModeLocked !== null}
              >
                <legend>Control mode</legend>
                <div className="control-mode-toggle">
                  {CONTROL_MODES.map(({ value, label, Icon }) => (
                    <label key={value} className="control-mode-option">
                      <input
                        className="sr-only"
                        type="radio"
                        name="control-mode"
                        value={value}
                        checked={controlMode === value}
                        disabled={manualOnly && value === "agent"}
                        onChange={() => selectControlMode(value)}
                      />
                      <span className="control-mode-label">
                        <Icon
                          className="control-mode-icon"
                          aria-hidden="true"
                        />
                        {label}
                      </span>
                    </label>
                  ))}
                </div>
              </fieldset>
              {manualOnly ? (
                <Alert
                  id="control-mode-hint"
                  role="status"
                  variant="destructive"
                >
                  <TriangleAlertIcon aria-hidden="true" />
                  <p className="m-0">
                    <strong>Agent disabled.</strong> The viewer was started with{" "}
                    <code>--manual-control-only-no-agent</code>, which is only
                    for debugging manual control. To use the agent, set{" "}
                    <code>OPENAI_API_KEY</code>, <code>ANTHROPIC_API_KEY</code>,
                    or <code>GEMINI_API_KEY</code> and restart the viewer
                    without that flag.
                  </p>
                </Alert>
              ) : (
                <p
                  id="control-mode-hint"
                  role="status"
                  className="m-0 text-xs text-muted-foreground"
                >
                  {controlModeLocked ??
                    "Choose a control mode, then enable the arm."}
                </p>
              )}
            </div>
            <ArmControl
              stationState={connected ? stationState : null}
              armName={armName}
              onSelectArm={(name) => {
                stopFollowing();
                setTarget(null);
                setProjection(null);
                setSelectedArm(name);
              }}
              runActive={
                startingRun ||
                (runActive &&
                  !(
                    run?.phase.state === "stopping" &&
                    connected &&
                    stationState?.lease === null
                  ))
              }
              manualSelected={controlMode === "manual"}
              onManualControlStarted={onManualControlStarted}
              onStopFollowing={stopFollowing}
              onPowerTransition={setChangingArmPower}
            />
            {!manualOnly && (
              <div hidden={controlMode !== "agent"}>
                <AgentControl
                  agent={agent}
                  startingRun={startingRun}
                  onStartRun={startRun}
                  runError={runError}
                  armName={armName}
                  modelSelection={preferences.agentModelSelection}
                  onModelSelectionChange={(agentModelSelection) =>
                    updatePreferences({ ...preferences, agentModelSelection })
                  }
                  onStopRun={stopRun}
                  stoppingRun={stoppingRun}
                  stationState={connected ? stationState : null}
                  cameraNames={cameraNames}
                />
              </div>
            )}
            <div hidden={controlMode !== "manual"}>
              {connected ? (
                <ManualControl
                  key={armName}
                  motionLimits={preferences.motionLimits}
                  armName={armName}
                  changingArmPower={changingArmPower}
                  active={controlMode === "manual"}
                  stationState={stationState}
                  target={target}
                  setTarget={setTarget}
                  projection={projection}
                  setProjection={setProjection}
                  projectionValidity={
                    projection !== null &&
                    projectionEvaluation?.projection === projection
                      ? projectionEvaluation.valid
                      : null
                  }
                  dragging={dragging}
                  globalStopHandler={globalStopHandler}
                />
              ) : (
                <p className="manual-unavailable">
                  Connect to the station to use manual controls.
                </p>
              )}
            </div>
            {renderAdditionalControls?.(run)}
          </Card>
        </div>
        <details
          className="telemetry-panel"
          onToggle={(event) => setTelemetryOpen(event.currentTarget.open)}
        >
          <summary id="workspace-telemetry" className="workspace-jump-target">
            Joint telemetry <span>Last 30 seconds</span>
          </summary>
          <LiveTelemetry
            visible={telemetryOpen}
            showTarget={preferences.showTargetJoints}
            showCommanded={preferences.showCommandedJoints}
          />
        </details>
        <details className="run-diagnostics">
          <summary id="workspace-activity" className="workspace-jump-target">
            Run activity <span>Prompt, settings, memory and traces</span>
          </summary>
          <RunPanels
            agentRun={run}
            spans={liveRun?.spans ?? EMPTY_SPANS}
            memory={liveRun?.memory ?? null}
            usage={liveRun?.usage ?? null}
            time={currentTime}
            onSeek={noSeek}
            live
          />
        </details>
      </main>
      {page === "settings" && (
        <SettingsPage
          preferences={preferences}
          update={updatePreferences}
          arms={connected ? (stationState?.arms ?? null) : null}
          agent={agent}
          section={settingsSection}
        />
      )}
    </ViewerShell>
  );
}

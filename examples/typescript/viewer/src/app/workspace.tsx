import type { StationState } from "@hebbian/robot-use-client";
import { apiSchemas } from "@hebbian/robot-use-client";
import { useMutation, useQuery } from "@tanstack/react-query";
import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { z } from "zod";
import { Alert } from "@/components/ui/alert.js";
import { Badge } from "@/components/ui/badge.js";
import { Button } from "@/components/ui/button.js";
import { Card, CardHeader } from "@/components/ui/card.js";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "@/components/ui/dialog.js";
import {
  Empty,
  EmptyDescription,
  EmptyHeader,
  EmptyTitle,
} from "@/components/ui/empty.js";
import { Input } from "@/components/ui/input.js";
import {
  Tabs,
  TabsContent,
  TabsList,
  TabsTrigger,
} from "@/components/ui/tabs.js";
import { agentApi, resolveAgentUrl } from "../agent-api.js";
import { stationJson } from "../api.js";
import { errorMessage } from "../errors.js";
import { AgentControl } from "../features/agent/agent-control.js";
import { CameraView, useCameraRoom } from "../features/cameras/live-cameras.js";
import { ArmControl } from "../features/control/arm-control.js";
import { ControllerRecovery } from "../features/control/controller-recovery.js";
import { ManualControl } from "../features/control/manual-control.js";
import {
  RecordingControls,
  useControlRecording,
} from "../features/recordings/recording-controls.js";
import {
  RobotScene,
  type SceneProjection,
} from "../features/robot-scene/robot-scene.js";
import { convertLiveRun, RunPanels } from "../features/run/run-panels.js";
import {
  SettingsPage,
  useViewerPreferences,
} from "../features/settings/settings-page.js";
import { LiveTelemetry } from "../features/telemetry/joint-charts.js";
import type { LiveRun as ViewerRun } from "../generated/live-run-schema.js";
import { armControl, manualLease } from "../manual-lease.js";
import { isRunActive, type Run } from "../run-logic.js";
import { ViewerShell } from "../shared/navigation.js";
import { useStationState } from "../use-station-state.js";
import { stationConnection } from "../viewer-connection.js";
import "./ui.css";
import "./workspace.css";

type Position = [number, number, number];
type Page = "workspace" | "settings";
type ControlTab = "agent" | "manual";
const EMPTY_SPANS: ViewerRun["spans"] = [];
const noSeek = (): void => {};

function currentPage(): Page {
  return location.hash === "#settings" ? "settings" : "workspace";
}

const mediaProgramSchema = z.discriminatedUnion("state", [
  z.object({ state: z.literal("running") }),
  z.object({ state: z.literal("not_configured") }),
  z.object({ state: z.literal("restarting"), last_exit: z.string() }),
  z.object({ state: z.literal("unavailable"), reason: z.string() }),
]);
const mediaStatusSchema = z.object({
  programs: z.object({
    serve: mediaProgramSchema,
    publish: mediaProgramSchema,
  }),
});

function useMediaStatus(): string {
  const query = useQuery({
    queryKey: ["media-status"],
    queryFn: async () =>
      mediaStatusSchema.parse(await stationJson("api/media-status")),
    refetchInterval: 5000,
    refetchIntervalInBackground: true,
    retry: false,
  });
  if (query.isError || !query.data) return "";
  const status = query.data.programs;
  return (["serve", "publish"] as const)
    .flatMap((name) => {
      const program = status[name];
      return program.state === "restarting"
        ? [`Media ${name} restarting: ${program.last_exit}`]
        : program.state === "unavailable"
          ? [`Media ${name} unavailable: ${program.reason}`]
          : [];
    })
    .join(" · ");
}

const PhoneInvite = memo(function PhoneInvite() {
  const [label, setLabel] = useState("");
  const cameraLabelField = useRef<HTMLInputElement>(null);
  const inviteMutation = useMutation({
    mutationFn: async (cameraLabel: string) =>
      apiSchemas.shape.PhoneInvite.parse(
        await stationJson("api/phone-invites", {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({
            label: cameraLabel
              .trim()
              .toLowerCase()
              .replaceAll(/[\s-]+/g, "_"),
          }),
        }),
      ),
  });
  const invite = inviteMutation.data;
  const message = inviteMutation.error
    ? `Could not create the code: ${errorMessage(inviteMutation.error)}. Use lowercase letters, digits and underscores.`
    : "";
  return (
    <Dialog modal={false}>
      <DialogTrigger render={<Button variant="outline" />}>
        Add a phone camera
      </DialogTrigger>
      <DialogContent
        className="max-sm:top-16 max-sm:max-h-[calc(100dvh-5rem)] max-sm:translate-y-0"
        initialFocus={cameraLabelField}
      >
        <DialogHeader>
          <DialogTitle>Add a phone camera</DialogTitle>
          <DialogDescription>
            Generate a QR code for your phone.
          </DialogDescription>
        </DialogHeader>
        <form
          className="grid gap-3"
          onSubmit={(event) => {
            event.preventDefault();
            if (!inviteMutation.isPending) inviteMutation.mutate(label);
          }}
        >
          <label htmlFor="phone-camera-label">
            Camera location
            <Input
              id="phone-camera-label"
              ref={cameraLabelField}
              value={label}
              onChange={(event) => setLabel(event.currentTarget.value)}
              placeholder="e.g. table_left"
              autoCapitalize="none"
              autoComplete="off"
              required
            />
          </label>
          <Button type="submit" disabled={inviteMutation.isPending}>
            {inviteMutation.isPending ? "Creating…" : "Show QR code"}
          </Button>
        </form>
        {message && <Alert variant="destructive">{message}</Alert>}
        {invite && (
          <div className="grid gap-2">
            <img
              className="mx-auto max-w-full bg-white"
              width={230}
              src={invite.qr_code_image}
              alt={`QR code for camera ${invite.camera_name}`}
            />
            <p role="status" className="m-0 text-muted-foreground">
              Scan with your phone, then start streaming. It appears as{" "}
              {invite.camera_name}. The code works for 12 hours.
            </p>
          </div>
        )}
      </DialogContent>
    </Dialog>
  );
});

function RecoveryControls({ state }: { state: StationState | null }) {
  const [messages, setMessages] = useState<Record<string, string>>({});
  const [pending, setPending] = useState<string | null>(null);
  async function recover(armName: string): Promise<void> {
    const arm = state?.arms.find((candidate) => candidate.arm === armName);
    if (arm?.mode !== "faulted") return;
    const fresh =
      arm.sampled_at_unix_seconds !== null &&
      Date.now() / 1000 - arm.sampled_at_unix_seconds < 1;
    if (!fresh) {
      setMessages((previous) => ({
        ...previous,
        [armName]:
          "Cannot clear: no recent motor feedback. Check power and USB, then restart the motor controller if needed.",
      }));
      return;
    }
    setPending(armName);
    try {
      const answer = await armControl(armName).clearFault();
      if (answer.status === "refused") throw new Error(answer.detail);
      setMessages((previous) => ({
        ...previous,
        [armName]: "Fault cleared. The arm remains disabled.",
      }));
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
          arm.sampled_at_unix_seconds !== null &&
          Date.now() / 1000 - arm.sampled_at_unix_seconds < 1;
        const message =
          (!fresh
            ? `${arm.arm}: Motor feedback is stale. Check the controller status above.`
            : arm.fault ||
              (arm.active_trip && `${arm.arm}: ${arm.active_trip}`)) ??
          messages[arm.arm];
        return (
          (message || arm.mode === "faulted") && (
            <Alert
              key={arm.arm}
              role="status"
              variant="destructive"
              className="flex flex-wrap items-center gap-3"
            >
              <p className="m-0">{message}</p>
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

export function WorkspaceApp() {
  const [page, setPage] = useState<Page>(currentPage);
  const [controlTab, setControlTab] = useState<ControlTab>("agent");
  const [selectedArm, setSelectedArm] = useState("");
  const recording = useControlRecording();
  const [telemetryOpen, setTelemetryOpen] = useState(false);
  const [manualMode, setManualMode] = useState<
    "unknown" | "disabled" | "enabled"
  >("unknown");
  const { stationState, connected } = useStationState();
  const [liveRun, setLiveRun] = useState<ViewerRun | null>(null);
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
  const [stopMessage, setStopMessage] = useState("");
  const globalStopHandler = useRef<(() => void) | null>(null);
  const [run, setRun] = useState<Run | null>(null);
  const [stoppingRun, setStoppingRun] = useState(false);
  const [preferences, updatePreferences] = useViewerPreferences();
  const agentUrl = resolveAgentUrl(preferences.agentServerUrl);
  const armNames = stationState?.arms.map((arm) => arm.arm) ?? [];
  const armName = armNames.includes(selectedArm)
    ? selectedArm
    : (armNames[0] ?? "");
  const stopFollowing = useCallback(() => globalStopHandler.current?.(), []);
  const cameras = useCameraRoom();
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
    const unsubscribeRun = stationConnection.onRun((run) =>
      setLiveRun(convertLiveRun(run)),
    );
    stationConnection.start();
    const interval = setInterval(() => setCurrentTime(Date.now() / 1000), 1000);
    const onHashChange = () => setPage(currentPage());
    window.addEventListener("hashchange", onHashChange);
    return () => {
      unsubscribeRun();
      clearInterval(interval);
      window.removeEventListener("hashchange", onHashChange);
    };
  }, []);
  useEffect(() => {
    let active = true;
    if (!connected) {
      setManualMode("unknown");
      setTarget(null);
      setProjection(null);
      return () => {
        active = false;
      };
    }
    void stationJson("api/control-mode")
      .then((value) => {
        const mode = apiSchemas.shape.ViewerControl.parse(value).mode;
        if (active)
          setManualMode(mode === "so101_manual" ? "enabled" : "disabled");
      })
      .catch(() => {
        if (active) setManualMode("unknown");
      });
    return () => {
      active = false;
    };
  }, [connected, setProjection]);

  async function stopRun(): Promise<void> {
    if (!run || stoppingRun) return;
    setStoppingRun(true);
    try {
      globalStopHandler.current?.();
      void manualLease.returnLease();
      // The agent server only signals its loop to exit; its lease ends with
      // the loop's connection, which stops the arms. Recording is untouched.
      if (isRunActive(run)) setRun(await agentApi.stopRun(agentUrl, run.id));
      setStopMessage("Stop requested. Check the arm and recording status.");
    } catch (error: unknown) {
      setStopMessage(`Could not stop the run: ${errorMessage(error)}`);
    } finally {
      setStoppingRun(false);
    }
  }
  async function stopRunForTakeover(): Promise<void> {
    const current = await agentApi.currentRun(agentUrl);
    if (!isRunActive(current) || !current) return;
    await agentApi.stopRun(agentUrl, current.id);
    const deadline = Date.now() + 20000;
    let pollMilliseconds = 10;
    while (true) {
      const latest = await agentApi.currentRun(agentUrl);
      if (!isRunActive(latest)) {
        setRun(latest);
        return;
      }
      if (Date.now() >= deadline)
        throw new Error("Timed out waiting for the agent run to stop.");
      await new Promise((resolve) =>
        window.setTimeout(resolve, pollMilliseconds),
      );
      pollMilliseconds = Math.min(100, pollMilliseconds * 2);
    }
  }
  return (
    <ViewerShell current={page} onStop={stopFollowing}>
      {cameras.recording && (
        <Badge
          className="fixed right-3 bottom-3 z-[100]"
          variant="destructive"
          role="status"
        >
          Recording
        </Badge>
      )}
      {stopMessage && (
        <Alert role="status" className="mx-6 w-auto">
          {stopMessage}
        </Alert>
      )}
      <ControllerRecovery />
      <RecoveryControls state={connected ? stationState : null} />
      <div hidden={page !== "workspace"} className="workspace-page">
        <div className="workspace-layout">
          <main className="workspace-visuals">
            <div className="workspace-heading">
              <div>
                <h1>Workspace</h1>
                <p>{cameras.status}</p>
                {mediaStatus && <p role="status">{mediaStatus}</p>}
              </div>
              <PhoneInvite />
            </div>
            <div className="camera-grid">
              {cameras.feeds.map((feed) => (
                <CameraView
                  key={feed.name}
                  feed={feed}
                  showLabel={preferences.showCameraLabels}
                />
              ))}
              {cameras.feeds.length === 0 && (
                <Empty>
                  <EmptyHeader>
                    <EmptyTitle>No camera feeds</EmptyTitle>
                    <EmptyDescription>
                      Connect a camera or add a phone to see the workspace.
                    </EmptyDescription>
                  </EmptyHeader>
                </Empty>
              )}
            </div>
            <RobotScene
              jointNames={stationState?.joint_names ?? []}
              arms={
                stationState?.arms.map((arm) => ({
                  arm: arm.arm,
                  basePose: arm.base_pose,
                  measuredJoints: arm.measured_joints,
                  commandedJoints: arm.commanded_joints,
                  destinationJoints: arm.active_command?.target_joints ?? null,
                  destinationLimited:
                    arm.active_command?.path_limited != null ||
                    arm.active_command?.pose_limited != null,
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
          </main>
          <Card
            role="complementary"
            aria-label="Control"
            className="control-panel gap-0 py-0"
          >
            <CardHeader className="pt-4">
              <h2>Control</h2>
            </CardHeader>
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
                isRunActive(run) &&
                !(
                  run?.phase.state === "stopping" &&
                  connected &&
                  stationState?.lease === null
                )
              }
              onStopFollowing={stopFollowing}
            />
            <RecordingControls
              recording={recording}
              run={run}
              runActive={isRunActive(run)}
            />
            <Tabs
              value={controlTab}
              onValueChange={(value) => {
                if (value === "agent" || value === "manual")
                  setControlTab(value);
              }}
            >
              <TabsList
                aria-label="Control mode"
                activateOnFocus
                variant="line"
                className="mx-4 w-auto"
              >
                <TabsTrigger value="agent">Agent</TabsTrigger>
                <TabsTrigger value="manual">Manual</TabsTrigger>
              </TabsList>
              <TabsContent value="agent" keepMounted>
                <AgentControl
                  startRecordingWithControl={recording.startWithControl}
                  agentUrl={agentUrl}
                  armName={armName}
                  onStopFollowing={stopFollowing}
                  onRunChange={setRun}
                  onStopRun={stopRun}
                  stoppingRun={stoppingRun}
                  stationState={connected ? stationState : null}
                  cameraNames={cameraNames}
                />
              </TabsContent>
              <TabsContent value="manual" keepMounted>
                {manualMode === "enabled" ? (
                  <ManualControl
                    key={armName}
                    armName={armName}
                    stopRunForTakeover={stopRunForTakeover}
                    stationState={connected ? stationState : null}
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
                    {manualMode === "disabled"
                      ? "Manual controls are disabled on this station. Start it with --manual-control to use them."
                      : "Manual control availability is unavailable. Check the station connection."}
                  </p>
                )}
              </TabsContent>
            </Tabs>
          </Card>
        </div>
        <details
          className="telemetry-panel"
          onToggle={(event) => setTelemetryOpen(event.currentTarget.open)}
        >
          <summary>
            Joint telemetry <span>Last 30 seconds</span>
          </summary>
          <LiveTelemetry
            visible={telemetryOpen}
            showTarget={preferences.showTargetJoints}
            showCommanded={preferences.showCommandedJoints}
          />
        </details>
        <details className="run-diagnostics" open>
          <summary>
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
      </div>
      {page === "settings" && (
        <SettingsPage
          preferences={preferences}
          update={updatePreferences}
          arms={connected ? (stationState?.arms ?? null) : null}
        />
      )}
    </ViewerShell>
  );
}

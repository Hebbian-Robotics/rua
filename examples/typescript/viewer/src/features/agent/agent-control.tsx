import type { StationState } from "@hebbian/robot-use-client";
import { useEffect, useState } from "react";
import { z } from "zod";
import { Alert } from "@/components/ui/alert.js";
import { Button } from "@/components/ui/button.js";
import { Input } from "@/components/ui/input.js";
import {
  NativeSelect,
  NativeSelectOption,
} from "@/components/ui/native-select.js";
import { Textarea } from "@/components/ui/textarea.js";
import {
  DEFAULT_REASONING_EFFORT,
  type ReasoningEffort,
} from "../../../../agent/src/providers/types.js";
import {
  providerSchema,
  reasoningEffortSchema,
} from "../../../shared/run-api.js";
import { agentApi } from "../../agent-api.js";
import { describeHolder, manualControlView } from "../../control-logic.js";
import { errorMessage } from "../../errors.js";
import { manualLease } from "../../manual-lease.js";
import {
  decideStart,
  isRunActive,
  type Run,
  runStatusLine,
} from "../../run-logic.js";
import { useAgentRun } from "./use-agent-run.js";

type RunOperation = "starting" | null;

export function AgentControl({
  stationState,
  agentUrl,
  cameraNames,
  onRunChange,
  armName,
  onStopFollowing,
  onStopRun,
  stoppingRun,
  startRecordingWithControl,
}: {
  startRecordingWithControl: () => Promise<void>;
  agentUrl: string;
  stationState: StationState | null;
  cameraNames: string[];
  onRunChange: (run: Run | null) => void;
  armName: string;
  onStopFollowing: () => void;
  onStopRun: () => Promise<void>;
  stoppingRun: boolean;
}) {
  const [task, setTask] = useState(() => {
    try {
      return localStorage.getItem("hebbian-agent-task:v1") ?? "";
    } catch {
      return "";
    }
  });
  const [selectedCameras, setSelectedCameras] = useState<
    Record<string, "required" | "optional" | "off">
  >({});
  const {
    run,
    setRun,
    status: agentStatus,
    providers,
    connectionError,
    providersError,
  } = useAgentRun(agentUrl);
  const [provider, setProvider] = useState("");
  const [model, setModel] = useState("");
  const [reasoning, setReasoning] = useState<ReasoningEffort>(
    DEFAULT_REASONING_EFFORT,
  );
  const [operation, setOperation] = useState<RunOperation>(null);
  const [message, setMessage] = useState("");
  const armNames = stationState?.arms.map((arm) => arm.arm) ?? [];
  const selectedArmState = stationState?.arms.find(
    (arm) => arm.arm === armName,
  );
  const currentProvider =
    providers?.find((entry) => entry.provider === provider) ?? providers?.[0];
  const form = {
    task,
    arm: armName,
    cameras: cameraNames
      .filter((name) => (selectedCameras[name] ?? "required") === "required")
      .join(","),
    optionalCameras: cameraNames
      .filter((name) => selectedCameras[name] === "optional")
      .join(","),
    provider: currentProvider?.provider ?? "",
    model,
    reasoningEffort: reasoning,
  };
  const decision = decideStart(form, run, armNames, operation !== null);
  const active = isRunActive(run);
  const runStopping = stoppingRun || run?.phase.state === "stopping";
  const controlView = manualControlView(
    stationState?.lease ?? null,
    manualLease.holdsLease(),
    manualLease.label,
  );
  const controlHeldElsewhere = controlView.kind === "elsewhere";
  const settingsLocked = operation !== null || active;
  const canStart =
    decision.kind === "allowed" &&
    agentStatus === "reachable" &&
    Boolean(providers?.length) &&
    !controlHeldElsewhere &&
    selectedArmState?.mode === "holding";

  useEffect(() => {
    onRunChange(run);
  }, [run, onRunChange]);

  async function startRun(): Promise<void> {
    const result = decideStart(form, run, armNames, operation !== null);
    if (result.kind === "blocked" || !canStart) return;
    setOperation("starting");
    setMessage("");
    try {
      await startRecordingWithControl();
      if (manualLease.holdsLease()) {
        onStopFollowing();
        await manualLease.returnLease();
      }
      setRun(await agentApi.startRun(agentUrl, result.request));
    } catch (error: unknown) {
      setMessage(`Could not start the run: ${errorMessage(error)}`);
    } finally {
      setOperation(null);
    }
  }

  return (
    <section className="agent-control" aria-label="Agent control">
      <h2>Run a task</h2>
      <p>
        The agent holds control until the run ends. Stop ends the run and halts
        the arm.
      </p>
      <label htmlFor="agent-task">
        Task
        <Textarea
          id="agent-task"
          value={task}
          disabled={settingsLocked}
          onChange={(event) => {
            const taskDraft = event.currentTarget.value;
            setTask(taskDraft);
            try {
              localStorage.setItem("hebbian-agent-task:v1", taskDraft);
            } catch {
              // Keep editing usable when storage is blocked or full.
            }
          }}
          placeholder="Describe what the robot should do"
        />
      </label>
      {agentStatus === "unreachable" && (
        <Alert variant="destructive" role="status">
          <p>
            Could not connect to the agent server at <code>{agentUrl}</code>.
            Start the agent server on the machine that holds the model keys:
          </p>
          <pre>pnpm start:agent</pre>
          <p>
            Check the address under Settings and the server's allowed origins.
          </p>
          {connectionError && <p>Details: {connectionError}</p>}
        </Alert>
      )}
      <details>
        <summary>Agent settings</summary>
        <label htmlFor="agent-provider">
          Provider
          <NativeSelect
            id="agent-provider"
            value={currentProvider?.provider ?? ""}
            disabled={settingsLocked}
            onChange={(event) => {
              setProvider(providerSchema.parse(event.currentTarget.value));
              setModel("");
            }}
          >
            {providers?.map((entry) => (
              <NativeSelectOption key={entry.provider} value={entry.provider}>
                {entry.provider}
              </NativeSelectOption>
            ))}
          </NativeSelect>
        </label>
        <p>
          {providersError
            ? `Could not load model providers: ${providersError}`
            : providers === null
              ? "Checking model credentials…"
              : providers.length
                ? "Providers with a key on the agent server are shown."
                : "No model API keys configured on the agent server."}
        </p>
        <label htmlFor="agent-model">
          Model ID
          <Input
            id="agent-model"
            value={model}
            disabled={settingsLocked}
            onChange={(event) => setModel(event.currentTarget.value)}
            placeholder={currentProvider?.defaultModel ?? "Provider default"}
          />
        </label>
        <label htmlFor="agent-reasoning">
          Reasoning effort
          <NativeSelect
            id="agent-reasoning"
            value={reasoning}
            disabled={settingsLocked}
            onChange={(event) =>
              setReasoning(
                reasoningEffortSchema.parse(event.currentTarget.value),
              )
            }
          >
            <NativeSelectOption value="low">Low</NativeSelectOption>
            <NativeSelectOption value="medium">Medium</NativeSelectOption>
            <NativeSelectOption value="high">High</NativeSelectOption>
            <NativeSelectOption value="xhigh">Extra high</NativeSelectOption>
          </NativeSelect>
        </label>
        <fieldset disabled={settingsLocked}>
          <legend>Cameras for the agent</legend>
          {cameraNames.length === 0 ? (
            <p>No cameras streaming; the agent receives state only.</p>
          ) : (
            cameraNames.map((name) => (
              <label
                key={name}
                htmlFor={`agent-camera-${encodeURIComponent(name)}`}
              >
                {name}
                <NativeSelect
                  id={`agent-camera-${encodeURIComponent(name)}`}
                  value={selectedCameras[name] ?? "required"}
                  onChange={(event) => {
                    const choice = z
                      .enum(["required", "optional", "off"])
                      .parse(event.currentTarget.value);
                    setSelectedCameras((previous) => ({
                      ...previous,
                      [name]: choice,
                    }));
                  }}
                >
                  <NativeSelectOption value="required">
                    Required
                  </NativeSelectOption>
                  <NativeSelectOption value="optional">
                    Optional
                  </NativeSelectOption>
                  <NativeSelectOption value="off">Off</NativeSelectOption>
                </NativeSelect>
              </label>
            ))
          )}
        </fieldset>
      </details>
      <div className="control-actions">
        <Button
          disabled={!canStart || settingsLocked}
          onClick={() => void startRun()}
        >
          Start task
        </Button>
        {active && (
          <Button
            variant="destructive"
            disabled={runStopping}
            onClick={() => void onStopRun()}
          >
            {runStopping ? (
              <>
                <progress aria-label="Stopping run" className="h-2 w-6" />
                Stopping run…
              </>
            ) : (
              "Stop run"
            )}
          </Button>
        )}
      </div>
      {controlView.kind === "elsewhere" && !active && (
        <p role="status">
          {describeHolder(controlView.heldBy)} already holds control. Release
          control there before starting a task.
        </p>
      )}
      {!canStart && (
        <p>
          {selectedArmState?.mode === "disabled" ||
          selectedArmState?.mode === "reconnected"
            ? "Enable the selected arm above before starting a task."
            : providers === null
              ? "Checking model credentials…"
              : !providers.length
                ? "Configure a model API key on the agent server."
                : selectedArmState?.mode !== "holding"
                  ? "Wait for the selected arm to hold before starting a task."
                  : decision.kind === "blocked"
                    ? decision.reason
                    : ""}
        </p>
      )}
      <p role="status">{message || runStatusLine(run, Date.now() / 1000)}</p>
    </section>
  );
}

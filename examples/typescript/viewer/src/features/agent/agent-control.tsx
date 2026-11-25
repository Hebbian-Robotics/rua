import type { StationState } from "hebbian-rua";
import { ImagePlus } from "lucide-react";
import { useRef, useState } from "react";
import { z } from "zod";
import { Button } from "@/components/ui/button.js";
import { Input } from "@/components/ui/input.js";
import {
  NativeSelect,
  NativeSelectOption,
} from "@/components/ui/native-select.js";
import { Textarea } from "@/components/ui/textarea.js";
import {
  DEFAULT_INFERENCE_MODE,
  DEFAULT_REASONING_EFFORT,
} from "../../../../agent/src/providers/types.js";
import {
  inferenceModeSchema,
  type PromptImage,
  providerSchema,
  reasoningEffortSchema,
} from "../../../shared/run-api.js";
import { describeHolder, manualControlView } from "../../control-logic.js";
import { errorMessage } from "../../errors.js";
import { manualLease } from "../../manual-lease.js";
import {
  chosenLessons,
  decideStart,
  inferenceModeOptions,
  isRunActive,
  type LessonEdits,
  type RunFormValues,
  type RunRequest,
  runStatusLine,
} from "../../run-logic.js";
import type { AgentModelSelection } from "../settings/preferences.js";
import {
  JPEG_DATA_URL_PREFIX,
  PROMPT_IMAGE_TYPES,
  preparePromptImage,
} from "./prompt-image.js";
import { LessonsPicker, RunFeedback, useTaskLessons } from "./run-feedback.js";
import type { useAgentRun } from "./use-agent-run.js";

const cameraModeSchema = z.enum(["required", "optional", "off"]);
const CUSTOM_MODEL_OPTION = "__custom__";

export function AgentControl({
  stationState,
  cameraNames,
  agent,
  startingRun,
  onStartRun,
  runError,
  armName,
  modelSelection,
  onModelSelectionChange,
  onStopRun,
  stoppingRun,
}: {
  stationState: StationState | null;
  cameraNames: string[];
  agent: ReturnType<typeof useAgentRun>;
  startingRun: boolean;
  onStartRun: (request: RunRequest) => Promise<void>;
  runError: string;
  armName: string;
  modelSelection: AgentModelSelection;
  onModelSelectionChange: (selection: AgentModelSelection) => void;
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
  const [reasoning, setReasoning] = useState(DEFAULT_REASONING_EFFORT);
  const promptImagePicker = useRef<HTMLInputElement>(null);
  const [promptImage, setPromptImage] = useState<PromptImage | null>(null);
  const [preparingPromptImage, setPreparingPromptImage] = useState(false);
  const [promptImageError, setPromptImageError] = useState("");
  const [preferredInferenceMode, setPreferredInferenceMode] = useState(
    DEFAULT_INFERENCE_MODE,
  );
  const [cameras, setCameras] = useState<
    Record<string, z.infer<typeof cameraModeSchema>>
  >({});
  const [lessonEdits, setLessonEdits] = useState<LessonEdits>({});
  const { run, status, providers, providersError } = agent;
  const { lessons, ready: lessonsReady } = useTaskLessons(
    agent.loadLessons,
    task,
    status === "reachable",
    run?.feedback?.savedAt,
  );
  const selectedProvider = modelSelection?.provider;
  const currentProvider = providers?.find(
    ({ provider }) => provider === selectedProvider,
  );
  const customModelSelected = modelSelection?.custom === true;
  const presetModel =
    currentProvider?.models.find(({ id }) => id === modelSelection?.model)
      ?.id ?? "";
  const model = customModelSelected
    ? (modelSelection.model ?? "")
    : presetModel;
  const inferenceModes = inferenceModeOptions(currentProvider?.provider);
  // A mode the current provider lacks, such as ultrafast after leaving OpenAI, falls back to standard.
  const inferenceMode = inferenceModes.some(
    ({ value }) => value === preferredInferenceMode,
  )
    ? preferredInferenceMode
    : DEFAULT_INFERENCE_MODE;
  const armNames = stationState?.arms.map((arm) => arm.arm) ?? [];
  const selectedArmState = stationState?.arms.find(
    (arm) => arm.arm === armName,
  );
  const form: RunFormValues = {
    task,
    arm: armName,
    cameras: cameraNames
      .filter((name) => (cameras[name] ?? "required") === "required")
      .join(","),
    optionalCameras: cameraNames
      .filter((name) => cameras[name] === "optional")
      .join(","),
    provider: selectedProvider ?? "",
    model,
    reasoningEffort: reasoning,
    inferenceMode,
  };
  const decision = decideStart(form, run, armNames, startingRun);
  const active = isRunActive(run);
  const runStopping = stoppingRun || run?.phase.state === "stopping";
  const controlView = manualControlView(
    stationState?.lease ?? null,
    manualLease.holdsLease(),
    manualLease.label,
  );
  const settingsLocked = startingRun || active;
  async function attachPromptImage(file: File): Promise<void> {
    setPreparingPromptImage(true);
    setPromptImageError("");
    try {
      setPromptImage(await preparePromptImage(file));
    } catch (error: unknown) {
      setPromptImageError(errorMessage(error));
    } finally {
      setPreparingPromptImage(false);
    }
  }
  let unavailable = "";
  if (!stationState) {
    unavailable = "Connect to the station to start a task.";
  } else if (status === "unreachable") {
    unavailable = "Agent server unavailable.";
  } else if (providersError) {
    unavailable = `Could not load model providers: ${providersError}`;
  } else if (providers === null) {
    unavailable = "Checking model providers…";
  } else if (!providers.some(({ available }) => available)) {
    unavailable = "No model API keys configured on the agent server.";
  } else if (selectedProvider && !currentProvider?.available) {
    unavailable = "The selected model provider is unavailable.";
  } else if (controlView.kind === "elsewhere") {
    unavailable = `${describeHolder(controlView.heldBy)} holds control. Release it there to start a task.`;
  } else if (selectedArmState?.mode !== "holding") {
    unavailable =
      selectedArmState?.mode === "disabled" ||
      selectedArmState?.mode === "reconnected"
        ? "Enable the arm to start a task."
        : "Wait for the arm to hold before starting a task.";
  } else if (task.trim() && decision.kind === "blocked") {
    unavailable = decision.reason;
  } else if (!lessonsReady) {
    unavailable = "Checking lessons from earlier runs…";
  }
  return (
    <section className="agent-control" aria-label="Agent control">
      <form
        aria-label="Run a task"
        className="grid gap-3"
        onSubmit={(event) => {
          event.preventDefault();
          if (
            !preparingPromptImage &&
            !unavailable &&
            decision.kind === "allowed"
          ) {
            const kept = chosenLessons(lessons, lessonEdits);
            // Choices apply to one run; the next start reviews lessons afresh.
            setLessonEdits({});
            void onStartRun({
              ...decision.request,
              lessons: kept,
              ...(promptImage ? { promptImage } : {}),
            });
          }
        }}
      >
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
        {!settingsLocked && (
          <LessonsPicker
            lessons={lessons}
            edits={lessonEdits}
            onEditsChange={setLessonEdits}
          />
        )}
        <div className="grid gap-2">
          <input
            ref={promptImagePicker}
            type="file"
            accept={PROMPT_IMAGE_TYPES}
            hidden
            onChange={(event) => {
              const file = event.currentTarget.files?.[0];
              event.currentTarget.value = "";
              if (file) void attachPromptImage(file);
            }}
          />
          <Button
            className="w-fit"
            type="button"
            variant="outline"
            disabled={settingsLocked || preparingPromptImage}
            onClick={() => promptImagePicker.current?.click()}
          >
            <ImagePlus aria-hidden="true" />
            {promptImage ? "Replace image" : "Attach image"}
          </Button>
          {promptImage && (
            <div className="flex min-w-0 items-center gap-2">
              <img
                className="h-12 w-12 shrink-0 rounded object-contain"
                src={JPEG_DATA_URL_PREFIX + promptImage.jpegBase64}
                alt=""
              />
              <span className="min-w-0 break-all">{promptImage.filename}</span>
              <Button
                type="button"
                variant="ghost"
                disabled={settingsLocked || preparingPromptImage}
                onClick={() => {
                  setPromptImage(null);
                  setPromptImageError("");
                }}
              >
                Remove image
              </Button>
            </div>
          )}
          <p className="empty:hidden" role="status">
            {preparingPromptImage ? "Preparing image…" : promptImageError}
          </p>
        </div>
        <fieldset
          className="grid gap-4 sm:grid-cols-2"
          disabled={settingsLocked}
        >
          <legend className="sr-only">Model selection</legend>
          <label htmlFor="agent-provider">
            Provider
            <NativeSelect
              id="agent-provider"
              name="provider"
              required
              value={selectedProvider ?? ""}
              onChange={(event) => {
                onModelSelectionChange({
                  provider: providerSchema.parse(event.currentTarget.value),
                  model: null,
                });
              }}
            >
              <NativeSelectOption value="" disabled>
                Select a provider
              </NativeSelectOption>
              {providers?.map(({ provider, label, available }) => (
                <NativeSelectOption
                  key={provider}
                  value={provider}
                  disabled={!available}
                >
                  {label}
                  {available ? "" : " (unavailable)"}
                </NativeSelectOption>
              ))}
            </NativeSelect>
          </label>
          <label htmlFor="agent-model">
            Model
            <NativeSelect
              id="agent-model"
              name="model"
              required
              disabled={!currentProvider?.available}
              value={customModelSelected ? CUSTOM_MODEL_OPTION : presetModel}
              onChange={(event) => {
                if (!currentProvider) return;
                const { value } = event.currentTarget;
                const provider = currentProvider.provider;
                onModelSelectionChange(
                  value === CUSTOM_MODEL_OPTION
                    ? { provider, model: null, custom: true }
                    : { provider, model: value },
                );
              }}
            >
              <NativeSelectOption value="" disabled>
                Select a model
              </NativeSelectOption>
              {currentProvider?.models.map(({ id, label }) => (
                <NativeSelectOption key={id} value={id}>
                  {label}
                </NativeSelectOption>
              ))}
              <NativeSelectOption value={CUSTOM_MODEL_OPTION}>
                Other
              </NativeSelectOption>
            </NativeSelect>
          </label>
          {customModelSelected && (
            <label className="sm:col-span-2" htmlFor="agent-custom-model">
              Model ID
              <Input
                id="agent-custom-model"
                name="customModel"
                type="text"
                required
                disabled={!currentProvider?.available}
                autoComplete="off"
                autoCapitalize="none"
                autoCorrect="off"
                spellCheck={false}
                value={model}
                onChange={(event) => {
                  onModelSelectionChange({
                    provider: modelSelection.provider,
                    model: event.currentTarget.value || null,
                    custom: true,
                  });
                }}
              />
            </label>
          )}
        </fieldset>
        <details className="control-options">
          <summary>Run options</summary>
          <fieldset className="grid gap-3" disabled={settingsLocked}>
            <legend className="sr-only">Run options</legend>
            <div className="grid gap-4 sm:grid-cols-2">
              <label htmlFor="agent-reasoning">
                Reasoning effort
                <NativeSelect
                  id="agent-reasoning"
                  value={reasoning}
                  onChange={(event) =>
                    setReasoning(
                      reasoningEffortSchema.parse(event.currentTarget.value),
                    )
                  }
                >
                  <NativeSelectOption value="low">Low</NativeSelectOption>
                  <NativeSelectOption value="medium">Medium</NativeSelectOption>
                  <NativeSelectOption value="high">High</NativeSelectOption>
                  <NativeSelectOption value="xhigh">
                    Extra high
                  </NativeSelectOption>
                </NativeSelect>
              </label>
              <label htmlFor="agent-inference-mode">
                Inference mode
                <NativeSelect
                  id="agent-inference-mode"
                  value={inferenceMode}
                  onChange={(event) =>
                    setPreferredInferenceMode(
                      inferenceModeSchema.parse(event.currentTarget.value),
                    )
                  }
                >
                  {inferenceModes.map(({ value, label }) => (
                    <NativeSelectOption key={value} value={value}>
                      {label}
                    </NativeSelectOption>
                  ))}
                </NativeSelect>
              </label>
            </div>
            <fieldset className="grid gap-2">
              <legend className="font-medium">Cameras for the agent</legend>
              {cameraNames.length === 0 && (
                <p>No live feeds. See Cameras in Workspace.</p>
              )}
              {cameraNames.map((name) => (
                <label
                  key={name}
                  className="setting-switch"
                  htmlFor={`agent-camera-${encodeURIComponent(name)}`}
                >
                  {name}
                  <NativeSelect
                    id={`agent-camera-${encodeURIComponent(name)}`}
                    value={cameras[name] ?? "required"}
                    onChange={(event) => {
                      const mode = cameraModeSchema.parse(
                        event.currentTarget.value,
                      );
                      setCameras((previous) => ({ ...previous, [name]: mode }));
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
              ))}
            </fieldset>
          </fieldset>
        </details>
        <div className="control-actions">
          {active ? (
            <Button
              type="button"
              variant="destructive"
              disabled={runStopping}
              onClick={() => void onStopRun()}
            >
              {runStopping ? "Stopping run…" : "Stop run"}
            </Button>
          ) : (
            <Button
              type="submit"
              disabled={
                preparingPromptImage ||
                !!unavailable ||
                decision.kind === "blocked"
              }
            >
              {startingRun ? "Starting task…" : "Start task"}
            </Button>
          )}
        </div>
      </form>
      {!settingsLocked && unavailable && (
        <p>
          {unavailable}{" "}
          {status === "unreachable" && (
            <a href="#settings/agent">Agent connection status</a>
          )}
        </p>
      )}
      <p className="empty:hidden" role="status">
        {runError}
      </p>
      {run && !runStopping && (
        <p className="first-letter:uppercase">
          {runStatusLine(run, Date.now() / 1000)}
        </p>
      )}
      {run && <RunFeedback run={run} saveFeedback={agent.saveFeedback} />}
    </section>
  );
}

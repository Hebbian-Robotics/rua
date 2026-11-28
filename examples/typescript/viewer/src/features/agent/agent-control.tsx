import {
  AssistantRuntimeProvider,
  AttachmentPrimitive,
  ComposerPrimitive,
  MessageNotSentError,
  type ThreadMessageLike,
  ThreadPrimitive,
  useExternalStoreRuntime,
} from "@assistant-ui/react";
import type { StationState } from "hebbian-rua";
import { ArrowUp, ImagePlus, Square } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { z } from "zod";
import { Button } from "@/components/ui/button.js";
import { Input } from "@/components/ui/input.js";
import {
  NativeSelect,
  NativeSelectOption,
} from "@/components/ui/native-select.js";
import { Tooltip } from "@/components/ui/tooltip.js";
import {
  DEFAULT_INFERENCE_MODE,
  DEFAULT_REASONING_EFFORT,
} from "../../../../agent/src/providers/types.js";
import {
  inferenceModeSchema,
  MAX_RUN_REQUEST_BYTES,
  providerSchema,
  reasoningEffortSchema,
} from "../../../shared/run-api.js";
import { describeHolder, manualControlView } from "../../control-logic.js";
import { errorMessage } from "../../errors.js";
import { manualLease } from "../../manual-lease.js";
import {
  decideStart,
  inferenceModeOptions,
  isRunActive,
  type RunFormValues,
  type RunRequest,
  runStatusLine,
} from "../../run-logic.js";
import type { TraceSpan } from "../../span-time.js";
import type { AgentModelSelection } from "../settings/preferences.js";
import {
  JPEG_DATA_URL_PREFIX,
  PROMPT_IMAGE_TYPES,
  preparePromptImage,
} from "./prompt-image.js";
import { TaskThread, taskMessages } from "./task-thread.js";
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
  runSpans,
  showRunActivity,
  onClearActivity,
  onViewRunDetails,
}: {
  stationState: StationState | null;
  cameraNames: string[];
  agent: ReturnType<typeof useAgentRun>;
  startingRun: boolean;
  onStartRun: (request: RunRequest) => Promise<boolean>;
  runError: string;
  armName: string;
  modelSelection: AgentModelSelection;
  onModelSelectionChange: (selection: AgentModelSelection) => void;
  onStopRun: () => Promise<void>;
  stoppingRun: boolean;
  runSpans: TraceSpan[];
  showRunActivity: boolean;
  onClearActivity?: () => void;
  onViewRunDetails?: () => void;
}) {
  const [task, setTask] = useState(() => {
    try {
      return localStorage.getItem("hebbian-agent-task:v1") ?? "";
    } catch {
      return "";
    }
  });
  const lastCompositionEnd = useRef<number | null>(null);
  const [reasoning, setReasoning] = useState(DEFAULT_REASONING_EFFORT);
  const [preparingImages, setPreparingImages] = useState(0);
  const preparingPromptImage = preparingImages > 0;
  const [promptImageError, setPromptImageError] = useState("");
  const [preferredInferenceMode, setPreferredInferenceMode] = useState(
    DEFAULT_INFERENCE_MODE,
  );
  const [cameras, setCameras] = useState<
    Record<string, z.infer<typeof cameraModeSchema>>
  >({});
  const { run, status, providers, providersError } = agent;
  const selectedProvider = modelSelection?.provider;
  const currentProvider = providers?.find(
    ({ provider }) => provider === selectedProvider,
  );
  const keyMissing =
    currentProvider &&
    !currentProvider.available &&
    !agent.browserKey(currentProvider.provider);
  const customModelSelected = modelSelection?.custom === true;
  const presetModel =
    currentProvider?.models.find(({ id }) => id === modelSelection?.model)
      ?.id ?? "";
  const model = customModelSelected
    ? (modelSelection.model ?? "")
    : presetModel;
  const inferenceModes = inferenceModeOptions(currentProvider?.provider);
  // WHY: A mode the current provider lacks, such as ultrafast after leaving OpenAI, falls back to standard.
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
  const armNeedsEnable =
    selectedArmState?.mode === "disabled" ||
    selectedArmState?.mode === "reconnected";
  let unavailable = "";
  if (!stationState) {
    unavailable = "Connect to the station to start a task.";
  } else if (status === "unreachable") {
    unavailable = "Agent server unavailable.";
  } else if (providersError) {
    unavailable = `Could not load model providers: ${providersError}`;
  } else if (providers === null) {
    unavailable = "Checking model providers…";
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
  }
  const runtime = useExternalStoreRuntime<ThreadMessageLike>({
    messages: showRunActivity ? taskMessages(run, runSpans) : [],
    convertMessage: (message) => message,
    isRunning: active,
    isSendDisabled:
      preparingPromptImage ||
      !!keyMissing ||
      !!unavailable ||
      decision.kind === "blocked",
    onCancel: onStopRun,
    adapters: {
      attachments: {
        accept: PROMPT_IMAGE_TYPES,
        add: async ({ file }) => {
          setPreparingImages((count) => count + 1);
          setPromptImageError("");
          try {
            const image = await preparePromptImage(file);
            return {
              id: crypto.randomUUID(),
              type: "image",
              name: image.filename,
              contentType: "image/jpeg",
              file,
              content: [
                {
                  type: "image",
                  image: JPEG_DATA_URL_PREFIX + image.jpegBase64,
                },
              ],
              status: { type: "requires-action", reason: "composer-send" },
            };
          } catch (error: unknown) {
            setPromptImageError(errorMessage(error));
            throw error;
          } finally {
            setPreparingImages((count) => count - 1);
          }
        },
        remove: async () => {
          setPromptImageError("");
        },
        send: async (attachment) => ({
          ...attachment,
          content: attachment.content ?? [],
          status: { type: "complete" },
        }),
      },
    },
    onNew: async (message) => {
      const messageText = message.content
        .filter((part) => part.type === "text")
        .map((part) => part.text)
        .join("\n");
      const submission = decideStart(
        { ...form, task: messageText },
        run,
        armNames,
        startingRun,
      );
      if (submission.kind === "allowed") {
        const promptImages = (message.attachments ?? []).flatMap(
          (attachment) => {
            const image = attachment.content.find(
              (part) => part.type === "image",
            );
            return image?.image.startsWith(JPEG_DATA_URL_PREFIX)
              ? [
                  {
                    filename: attachment.name,
                    jpegBase64: image.image.slice(JPEG_DATA_URL_PREFIX.length),
                  },
                ]
              : [];
          },
        );
        const request = promptImages.length
          ? { ...submission.request, promptImages }
          : submission.request;
        const requestSize = new TextEncoder().encode(
          JSON.stringify(request),
        ).byteLength;
        if (requestSize > MAX_RUN_REQUEST_BYTES - 64 * 1024) {
          setPromptImageError(
            "The task and images are too large. Remove some images and try again.",
          );
          throw new MessageNotSentError();
        }
        const started = await onStartRun(request);
        if (!started) throw new MessageNotSentError();
        if (!runtime.thread.composer.getState().text) {
          runtime.thread.composer.setText(messageText);
        }
      }
    },
  });
  useEffect(() => {
    runtime.thread.composer.setText(task);
  }, [runtime, task]);
  return (
    <section className="agent-control" aria-label="Agent control">
      <AssistantRuntimeProvider runtime={runtime}>
        <ThreadPrimitive.Root>
          {showRunActivity && run && (
            <>
              <div className="flex items-center justify-between gap-2">
                <h3>Task activity</h3>
                <div className="flex flex-wrap gap-1">
                  <Button
                    type="button"
                    variant="ghost"
                    onClick={onViewRunDetails}
                  >
                    View run details
                  </Button>
                  <Button
                    type="button"
                    variant="ghost"
                    onClick={onClearActivity}
                  >
                    Clear activity
                  </Button>
                </div>
              </div>
              <TaskThread />
            </>
          )}
          <ComposerPrimitive.Root
            aria-label="Run a task"
            className="grid gap-3"
          >
            <label className="sr-only" htmlFor="agent-task">
              Task
            </label>
            <div className="grid gap-2 rounded-xl border border-border bg-background p-2 focus-within:ring-2 focus-within:ring-ring/50">
              <ComposerPrimitive.Input
                id="agent-task"
                value={task}
                className="min-h-20 resize-y border-0 bg-transparent shadow-none focus-visible:ring-0"
                disabled={settingsLocked}
                onChange={(event) => {
                  const taskDraft = event.currentTarget.value;
                  setTask(taskDraft);
                  try {
                    localStorage.setItem("hebbian-agent-task:v1", taskDraft);
                  } catch {
                    // WHY: Keep editing usable when storage is blocked or full.
                  }
                }}
                submitMode="enter"
                unstable_insertNewlineOnTouchEnter
                onCompositionEnd={(event) => {
                  lastCompositionEnd.current = event.timeStamp;
                }}
                onKeyDown={(event) => {
                  if (
                    event.key === "Enter" &&
                    lastCompositionEnd.current !== null &&
                    Math.abs(event.timeStamp - lastCompositionEnd.current) < 50
                  ) {
                    event.preventDefault();
                  }
                }}
                placeholder="Describe what the robot should do"
              />
              <ComposerPrimitive.Attachments>
                {({ attachment }) => (
                  <div className="flex min-w-0 items-center gap-2">
                    {attachment.content?.[0]?.type === "image" && (
                      <img
                        className="h-12 w-12 shrink-0 rounded object-contain"
                        src={attachment.content[0].image}
                        alt=""
                      />
                    )}
                    <span className="min-w-0 break-all">
                      <AttachmentPrimitive.Name />
                    </span>
                    <AttachmentPrimitive.Remove
                      type="button"
                      className="text-sm underline"
                      disabled={settingsLocked || preparingPromptImage}
                    >
                      Remove image
                    </AttachmentPrimitive.Remove>
                  </div>
                )}
              </ComposerPrimitive.Attachments>
              <p className="empty:hidden" role="status">
                {preparingPromptImage ? "Preparing image…" : promptImageError}
              </p>
              <div className="flex items-center justify-between gap-2">
                <Tooltip
                  text="Attach image"
                  disabled={settingsLocked || preparingPromptImage}
                  className="inline-flex"
                >
                  <ComposerPrimitive.AddAttachment
                    type="button"
                    className="inline-flex size-8 items-center justify-center rounded-lg hover:bg-muted disabled:opacity-50"
                    aria-label="Attach image"
                    disabled={settingsLocked || preparingPromptImage}
                    multiple
                  >
                    <ImagePlus aria-hidden="true" className="size-4" />
                  </ComposerPrimitive.AddAttachment>
                </Tooltip>
                {active ? (
                  <Tooltip
                    text={runStopping ? "Stopping run…" : "Stop run"}
                    disabled={runStopping}
                    className="inline-flex"
                  >
                    <ComposerPrimitive.Cancel
                      type="button"
                      className="inline-flex size-8 items-center justify-center rounded-lg bg-destructive-soft text-destructive disabled:opacity-50"
                      aria-label={runStopping ? "Stopping run" : "Stop run"}
                      disabled={runStopping}
                    >
                      <Square
                        aria-hidden="true"
                        className="size-4 fill-current"
                      />
                    </ComposerPrimitive.Cancel>
                  </Tooltip>
                ) : (
                  <Tooltip
                    text={
                      armNeedsEnable
                        ? "Enable the arm before starting a task."
                        : startingRun
                          ? "Starting task…"
                          : "Start task"
                    }
                    disabled={
                      preparingPromptImage ||
                      !!keyMissing ||
                      !!unavailable ||
                      decision.kind === "blocked"
                    }
                    className="inline-flex"
                  >
                    {armNeedsEnable ? (
                      <a
                        href="#enable-arm"
                        className="inline-flex size-8 cursor-default items-center justify-center rounded-lg bg-disabled !text-disabled-foreground no-underline"
                        aria-label="Enable the arm before starting a task"
                        aria-describedby="enable-arm-hint"
                      >
                        <ArrowUp aria-hidden="true" className="size-4" />
                      </a>
                    ) : (
                      <ComposerPrimitive.Send
                        className="inline-flex size-8 items-center justify-center rounded-lg bg-primary text-primary-foreground disabled:pointer-events-none disabled:bg-disabled disabled:text-disabled-foreground"
                        type="submit"
                        aria-label={
                          startingRun ? "Starting task" : "Start task"
                        }
                        aria-describedby={
                          armNeedsEnable ? "enable-arm-hint" : undefined
                        }
                        disabled={
                          preparingPromptImage ||
                          !!keyMissing ||
                          !!unavailable ||
                          decision.kind === "blocked"
                        }
                      >
                        <ArrowUp aria-hidden="true" className="size-4" />
                      </ComposerPrimitive.Send>
                    )}
                  </Tooltip>
                )}
              </div>
            </div>
            <p className="text-xs">
              Each task starts a new run; history is not carried over.
            </p>
            {!settingsLocked && armNeedsEnable && (
              <p id="enable-arm-hint">
                <a href="#enable-arm">Enable the arm</a> before starting a task.
              </p>
            )}
            <fieldset
              className="grid gap-4 border-0 sm:grid-cols-2"
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
                  {providers?.map(({ provider, label }) => (
                    <NativeSelectOption key={provider} value={provider}>
                      {label}
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
                  disabled={!currentProvider}
                  value={
                    customModelSelected ? CUSTOM_MODEL_OPTION : presetModel
                  }
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
              {keyMissing && (
                <p className="sm:col-span-2" role="status">
                  Add a {currentProvider.label} API key to run this agent.{" "}
                  <a href={`#settings/key-${currentProvider.provider}`}>
                    Set key
                  </a>
                </p>
              )}
              {customModelSelected && (
                <label className="sm:col-span-2" htmlFor="agent-custom-model">
                  Model ID
                  <Input
                    id="agent-custom-model"
                    name="customModel"
                    type="text"
                    required
                    disabled={!currentProvider}
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
              <fieldset
                className="grid gap-3 border-0"
                disabled={settingsLocked}
              >
                <legend className="sr-only">Run options</legend>
                <div className="grid gap-4 sm:grid-cols-2">
                  <label htmlFor="agent-reasoning">
                    Reasoning effort
                    <NativeSelect
                      id="agent-reasoning"
                      value={reasoning}
                      onChange={(event) =>
                        setReasoning(
                          reasoningEffortSchema.parse(
                            event.currentTarget.value,
                          ),
                        )
                      }
                    >
                      <NativeSelectOption value="low">Low</NativeSelectOption>
                      <NativeSelectOption value="medium">
                        Medium
                      </NativeSelectOption>
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
                <fieldset className="grid gap-2 border-0">
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
                          setCameras((previous) => ({
                            ...previous,
                            [name]: mode,
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
                  ))}
                </fieldset>
              </fieldset>
            </details>
          </ComposerPrimitive.Root>
        </ThreadPrimitive.Root>
      </AssistantRuntimeProvider>
      {!settingsLocked &&
        unavailable &&
        unavailable !== "Enable the arm to start a task." && (
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
    </section>
  );
}

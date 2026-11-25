import { match } from "ts-pattern";
import { apiSchemas } from "./generated/api-schemas.js";
import { modelOutcomes, robotToolContext } from "./generated/robot-tools.js";
import {
  defaultFormatter,
  type OutputFormatter,
  parseModelData,
  type RobotOutput,
} from "./output-format.js";
import { modelResponse } from "./response-format.js";
import { parseRobotToolCall } from "./robot-tool-input.js";
import type {
  ObservationText,
  RobotObservation,
  RobotToolCall,
  RobotToolResult,
  SequenceImageHistory,
  ToolCallWrapper,
} from "./robot-tool-types.js";
import { SequenceImages } from "./sequence-images.js";
import {
  type Lease,
  newOperationId,
  type StationConnection,
} from "./station-connection.js";
import {
  ArmRecoveryEndedError,
  LeaseLostError,
  StationRefusedError,
  StationTimeoutError,
} from "./station-errors.js";
import {
  type ActionSequenceRequest,
  type ArmState,
  type CommandAnswer,
  type CommandRecord,
  commandPlan,
  commandResult,
  commandWasLimited,
  type PreviewPose,
  type StationState,
} from "./station-types.js";
import {
  configurationFromCapabilities,
  createRobotToolCatalog,
  type RobotToolCatalogConfiguration,
  type ToolTransformation,
} from "./tool-catalog.js";

class RequiredCameraUnavailable extends Error {
  constructor(public readonly camera: string) {
    super(`Required camera ${camera} is unavailable`);
  }
}

class ObservationWaitTimedOut extends Error {}

class ObservationChanged extends Error {
  constructor(arm: string) {
    super(`Arm ${arm} changed during observation`);
  }
}

type ToolCompletion = "returned" | "incomplete";
type ToolInterruption =
  | "none"
  | "faulted"
  | "interrupted"
  | "faulted_and_interrupted";

function toolInterruption(
  faulted: boolean,
  interrupted: boolean,
): ToolInterruption {
  if (faulted) return interrupted ? "faulted_and_interrupted" : "faulted";
  return interrupted ? "interrupted" : "none";
}

function result(
  call: RobotToolCall,
  value: unknown,
  completion: ToolCompletion = "returned",
  options: {
    interruption?: ToolInterruption;
    cameras?: Record<string, Uint8Array>;
    sequenceImages?: SequenceImageHistory;
  } = {},
): RobotToolResult {
  const { interruption = "none", ...images } = options;
  const content = defaultFormatter({
    kind: "result",
    data: value,
    call,
    ...images,
  });
  return {
    call,
    cameras: content.cameras,
    sequenceImages: images.sequenceImages,
    text: content.text,
    data: value,
    get modelData() {
      return parseModelData(this.text);
    },
    isError: completion === "incomplete",
    halt:
      interruption === "faulted" || interruption === "faulted_and_interrupted",
    interrupted:
      interruption === "interrupted" ||
      interruption === "faulted_and_interrupted",
  };
}

function modelRecordRule(record: CommandRecord) {
  return modelOutcomes.commands[commandResult(record)?.outcome ?? "pending"];
}

function toolCommandResult(
  call: RobotToolCall,
  answer: CommandAnswer,
): RobotToolResult {
  if (answer.status === "refused")
    return result(call, answer, "incomplete", {
      interruption: toolInterruption(
        answer.reason === "faulted",
        ["stale_revision", "interlocked", "lease_held_elsewhere"].includes(
          answer.reason,
        ),
      ),
    });
  const records = answer.records.map(({ record }) => record);
  const outcomes = records.map((record) => modelRecordRule(record).class);
  const targets = new Set(
    records.map(
      (record) =>
        modelRecordRule(record)[
          commandWasLimited(record) ? "limited_target" : "target"
        ],
    ),
  );
  const status =
    modelOutcomes.status_priority.findLast((target) => targets.has(target)) ??
    "partial";
  const payloads = records;
  const payload =
    call.name === "move_arms"
      ? {
          group_id: commandPlan(records[0])?.group_id,
          status,
          commands: payloads,
        }
      : payloads.length === 1
        ? payloads[0]
        : payloads;
  return result(
    call,
    payload,
    status === "reached" ? "returned" : "incomplete",
    {
      interruption: toolInterruption(
        outcomes.includes("faulted"),
        outcomes.includes("interrupted"),
      ),
    },
  );
}

/** Interlocks other than a required camera's, which only the arm itself can clear. */
function armInterlocks(selected: ArmState): ArmState["interlocks"] {
  return selected.interlocks.filter(
    (interlock) => interlock.kind !== "required_camera_unavailable",
  );
}

const motionLimitKeys = [
  "max_joint_speed",
  "max_joint_acceleration",
  "max_joint_jerk",
] as const;

type MotionCaps = Pick<PreviewPose, (typeof motionLimitKeys)[number]>;

function capMotion(request: MotionCaps, limits: MotionCaps): MotionCaps {
  const capped: MotionCaps = {};
  for (const key of motionLimitKeys) {
    const limit = limits[key];
    const requested = request[key];
    if (limit != null)
      capped[key] = requested == null ? limit : Math.min(requested, limit);
  }
  return capped;
}

function capActionSequence(
  request: ActionSequenceRequest,
  limits: MotionCaps | undefined,
): ActionSequenceRequest {
  if (limits === undefined) return request;
  return {
    ...request,
    steps: request.steps.map((step) => ({
      ...step,
      arms: Object.fromEntries(
        Object.entries(step.arms).map(([arm, action]) => [
          arm,
          "goal" in action
            ? {
                ...action,
                ...capMotion(
                  {
                    ...action,
                    max_joint_speed:
                      action.max_joint_speed ?? action.goal.max_joint_speed,
                  },
                  limits,
                ),
              }
            : action,
        ]),
      ),
    })),
  };
}

export type RobotToolsOptions = {
  readonly connection: StationConnection;
  readonly lease: Lease;
  readonly arm: string;
  /** @deprecated Sequence images use the station’s configured media companion. */
  readonly mediaUrl?: string;
  readonly cameras?: readonly string[];
  readonly optionalCameras?: readonly string[];
  readonly signal?: AbortSignal;
  readonly motionLimits?: MotionCaps;
  readonly descriptions?: Readonly<Record<string, string>>;
} & RobotToolCatalogConfiguration;

type LeaseTracking = {
  isLost: () => boolean;
  close: () => void;
  sequenceImages: SequenceImages;
};
export type RobotToolView = ReturnType<typeof createRobotToolsCore> & {
  mapTools: (transform: ToolTransformation) => RobotToolView;
  /** Return a view sharing the lease; replaces any previous formatter. */
  withOutputFormatter: (formatter: OutputFormatter) => RobotToolView;
};

/** Model-facing tools backed by the station. Acquire the lease first. */
export function createRobotTools(options: RobotToolsOptions): RobotToolView {
  const catalog = createRobotToolCatalog(options);
  const sequenceImages = new SequenceImages();
  let leaseLost = false;
  const close = options.connection.onLeaseLost(() => {
    leaseLost = true;
  });
  try {
    return createRobotToolView(options, catalog, {
      isLost: () => leaseLost,
      close: () => {
        close();
        sequenceImages.clear();
      },
      sequenceImages,
    });
  } catch (error) {
    close();
    throw error;
  }
}

function createRobotToolView(
  options: RobotToolsOptions,
  catalog: ReturnType<typeof createRobotToolCatalog>,
  tracking: LeaseTracking,
  formatter: OutputFormatter = defaultFormatter,
): RobotToolView {
  const capabilities = catalog.capabilities;
  return {
    ...createRobotToolsCore(options, catalog, tracking, formatter),
    withOutputFormatter(nextFormatter) {
      return createRobotToolView(options, catalog, tracking, nextFormatter);
    },
    mapTools(transform) {
      const configuration = configurationFromCapabilities(
        transform(structuredClone(capabilities)),
      );
      const mappedOptions = { ...options, ...configuration };
      const mappedCatalog = createRobotToolCatalog(mappedOptions);
      return createRobotToolView(
        mappedOptions,
        mappedCatalog,
        tracking,
        formatter,
      );
    },
  };
}

function createRobotToolsCore(
  options: RobotToolsOptions,
  catalog: ReturnType<typeof createRobotToolCatalog>,
  tracking: LeaseTracking,
  formatter: OutputFormatter,
) {
  const { connection, lease, arm } = options;
  function delay(milliseconds: number): Promise<void> {
    return new Promise((resolve, reject) => {
      signal?.throwIfAborted();
      const timer = setTimeout(() => {
        signal?.removeEventListener("abort", abort);
        resolve();
      }, milliseconds);
      function abort(): void {
        clearTimeout(timer);
        reject(signal?.reason);
      }
      signal?.addEventListener("abort", abort, { once: true });
    });
  }

  const availableToolNames = new Map(
    catalog.tools.map(({ name }) => [name, name]),
  );
  const cameras = options.cameras ?? [];
  const optionalCameras = options.optionalCameras ?? [];
  if (
    new Set([...cameras, ...optionalCameras]).size !==
    cameras.length + optionalCameras.length
  )
    throw new Error("Camera names must be unique");
  const captureCameras = [...cameras, ...optionalCameras];
  async function refreshSequenceImages(id: string): Promise<void> {
    try {
      const deadline = AbortSignal.timeout(2000);
      const requestSignal = signal
        ? AbortSignal.any([signal, deadline])
        : deadline;
      const history = await connection.getSequenceCameraHistory?.(
        id,
        requestSignal,
      );
      if (history) tracking.sequenceImages.cache(id, history);
    } catch {
      // WHY: Optional camera evidence cannot change the operation's outcome.
    }
  }

  const signal = options.signal;
  const waitOptions = signal === undefined ? {} : { signal };

  function stationState(fresh = false): Promise<StationState> {
    return connection.waitForState(() => true, {
      ...waitOptions,
      fresh,
      timeoutMilliseconds: 10_000,
    });
  }

  function armOf(state: StationState): ArmState {
    const selected = state.arms.find((candidate) => candidate.arm === arm);
    if (!selected) throw new Error(`The station has no arm ${arm}`);
    return selected;
  }

  /** Throw when an interlock inhibits the arm; a camera's is allowed when asked. */
  function requireNoInterlock(
    selected: ArmState,
    allowCameraInterlock: boolean,
  ) {
    const blocking = allowCameraInterlock
      ? armInterlocks(selected)
      : selected.interlocks;
    if (blocking.length > 0)
      throw new Error(
        `Arm ${arm} is interlocked: ${blocking.map((interlock) => interlock.kind).join(", ")}`,
      );
  }

  async function captureObservation(
    text: ObservationText,
    mode: "immediate" | "allow_camera_interlock" | "ready",
  ): Promise<RobotObservation> {
    const selection = {
      arms: [],
      required_cameras: [...cameras],
      optional_cameras: [...optionalCameras],
    };
    const capture =
      mode === "ready"
        ? await lease.waitForObservation(selection, arm, 5000, signal)
        : await connection.captureObservation(selection, signal);
    switch (capture.status) {
      case "observation_required_camera_unavailable":
        throw new RequiredCameraUnavailable(capture.camera);
      case "observation_arm_disconnected":
        throw new Error(`Arm ${capture.arm} is disconnected`);
      case "observation_revision_changed":
        throw new ObservationChanged(capture.arm);
      case "observation_wait_timed_out":
        throw new ObservationWaitTimedOut();
      case "observation_recovery_ended":
        throw new ArmRecoveryEndedError(
          `Arm ${capture.arm} is ${capture.mode}`,
        );
      case "observation_captured": {
        const state = apiSchemas.shape.StationState.parse(capture.state);
        requireNoInterlock(armOf(state), mode === "allow_camera_interlock");
        const output: Extract<RobotOutput, { kind: "observation" }> = {
          kind: "observation",
          data: state,
          selectedArm: arm,
          notes: [
            ...(text.extraLines ?? []),
            ...(text.note ? [text.note] : []),
          ],
          cameras: Object.fromEntries(
            Object.entries(capture.cameras).map(([name, frame]) => [
              name,
              Uint8Array.from(atob(frame.jpeg_base64), (character) =>
                character.charCodeAt(0),
              ),
            ]),
          ),
          missingOptionalCameras: capture.missing_optional_cameras,
        };
        const content = formatter(structuredClone(output));
        return {
          data: state,
          text: content.text,
          cameras: content.cameras ?? {},
          revisions: Object.fromEntries(
            state.arms.map((candidate) => [candidate.arm, candidate.revision]),
          ),
        };
      }
    }
    throw new Error(
      `Unknown capture result ${JSON.stringify(capture satisfies never)}`,
    );
  }

  function observe(
    allowCameraInterlock = false,
    text: ObservationText = {},
  ): Promise<RobotObservation> {
    return captureObservation(
      text,
      allowCameraInterlock ? "allow_camera_interlock" : "immediate",
    );
  }

  /**
   * Wait through arm recovery and required-camera interlocks, then observe
   * again. Needs the lease; throws `LeaseLostError` once it is lost, and
   * `ArmRecoveryEndedError` when the arm is disabled or faulted.
   */
  async function observeWhenReady(
    text: ObservationText = {},
  ): Promise<RobotObservation> {
    while (true) {
      if (tracking.isLost() || !lease.isHeld()) throw new LeaseLostError();
      try {
        const observation = await captureObservation(text, "ready");
        const latest = armOf(await stationState());
        if (tracking.isLost() || !lease.isHeld()) throw new LeaseLostError();
        const revision = observation.revisions[arm];
        if (revision !== undefined && latest.revision <= revision)
          return observation;
      } catch (error) {
        if (!(error instanceof ObservationWaitTimedOut)) throw error;
      }
    }
  }

  /** Factual conventions and the current station's robot description. */
  async function context() {
    const state = await stationState();
    return {
      conventions: { ...robotToolContext },
      profile: state.robot,
      description: state.description,
      jointNames: state.joint_names,
      arms: state.arms.map((item) => ({
        name: item.arm,
        hasGripper: item.gripper_opening !== null,
      })),
      maxJointSpeedDegreesPerSecond: (state.max_joint_speed * 180) / Math.PI,
      jawClosingAxis: state.jaw_closing_axis,
      surfaceGuard: modelResponse(state.surface_guard),
      modelResultUnits:
        "Fields ending in _mm use millimeters and fields ending in _degrees use degrees; speeds use degrees per second in both inputs and outputs.",
    };
  }

  async function execute(
    calls: readonly RobotToolCall[],
    revisions?: Readonly<Record<string, number>>,
    wrap: ToolCallWrapper = (_call, run) => run(),
  ): Promise<RobotToolResult[]> {
    const parsed = calls.map((call) =>
      parseRobotToolCall(call, {
        arm,
        sequenceScope: catalog.sequenceScope,
        sequenceActions: catalog.sequenceActions,
        availableToolNames,
        revisions,
      }),
    );
    signal?.throwIfAborted();
    const outcomes = await Promise.allSettled(
      parsed.map(async (item) => {
        let started = false;
        const outcome = await wrap(item.call, async (traceContext) => {
          if (started)
            throw new Error("A tool wrapper may run its call only once");
          started = true;
          try {
            return await match(item)
              .with({ kind: "invalid" }, ({ call, error }) =>
                result(call, error, "incomplete"),
              )
              .with({ kind: "preview" }, async (item) => {
                const previewBatch = {
                  arm: item.arm,
                  poses: item.poses.map((pose) => ({
                    ...pose,
                    pose: {
                      ...pose.pose,
                      ...capMotion(pose.pose, options.motionLimits ?? {}),
                    },
                    id: newOperationId(),
                  })),
                };
                const response = await connection.previewPoses(previewBatch);
                const answersById = new Map(
                  response.results.map((candidate) => [
                    candidate.id,
                    candidate.answer,
                  ]),
                );
                return result(item.call, {
                  results: previewBatch.poses.map((candidate) => {
                    const answer = answersById.get(candidate.id);
                    if (answer === undefined)
                      throw new Error("Missing preview candidate result");
                    return { answer };
                  }),
                });
              })
              .with({ kind: "observe" }, async (item) => {
                await delay(item.wait * 1000);
                if (item.source === "last_sequence") {
                  const execution = tracking.sequenceImages.execution();
                  if (execution && !tracking.sequenceImages.snapshot())
                    await refreshSequenceImages(execution);
                  const history = tracking.sequenceImages.snapshot();
                  if (!history)
                    return result(
                      item.call,
                      "No sequence image history is available; none was captured or it expired.",
                      "incomplete",
                    );
                  return result(
                    item.call,
                    {
                      source: "last_sequence",
                      sequence_call_id: history.callId,
                      missed_samples: history.missedSamples,
                      truncated: history.truncated,
                      images: history.images.map((image) => ({
                        camera: image.camera,
                        elapsed_seconds: image.elapsedSeconds,
                        retrieved_between_unix_seconds:
                          image.retrievedBetweenUnixSeconds,
                        boundary: image.boundary,
                      })),
                    },
                    "returned",
                    { sequenceImages: history },
                  );
                }
                const observation = await observe();
                return result(item.call, observation.data, "returned", {
                  cameras: observation.cameras,
                });
              })
              .with({ kind: "rehome" }, async (item) => {
                return toolCommandResult(
                  item.call,
                  await lease
                    .arm(arm, { traceContext })
                    .rehome(revisions?.[arm]),
                );
              })
              .with(
                { kind: "message", message: { kind: "move_arms" } },
                async ({ call, message }) =>
                  toolCommandResult(
                    call,
                    await lease.runGroup(
                      message.command,
                      message.expected_revisions,
                      undefined,
                      traceContext,
                    ),
                  ),
              )
              .with(
                { kind: "message", message: { kind: "command" } },
                async ({ call, message }) =>
                  toolCommandResult(
                    call,
                    await lease.runCommand(
                      message.arm,
                      message.command,
                      message.expected_revision ?? undefined,
                      undefined,
                      traceContext,
                    ),
                  ),
              )
              .with(
                { kind: "message", message: { kind: "start_action_sequence" } },
                async ({ call, message }) => {
                  tracking.sequenceImages.clear();
                  const execution = await lease.startSequenceExecution(
                    capActionSequence(
                      message.action_sequence,
                      options.motionLimits,
                    ),
                    captureCameras,
                  );
                  const sequenceId = execution.id;
                  if (captureCameras.length)
                    tracking.sequenceImages.select(sequenceId, call.id);
                  const sequence = await execution.wait(waitOptions);
                  if (captureCameras.length)
                    await refreshSequenceImages(sequenceId);
                  const outcomes = sequence.steps
                    .flat()
                    .map(({ record }) => modelRecordRule(record).class);
                  const modes = (await stationState()).arms
                    .filter((candidate) =>
                      sequence.arms.includes(candidate.arm),
                    )
                    .map((candidate) => candidate.mode);
                  return result(
                    call,
                    sequence,
                    sequence.phase.state === "completed"
                      ? "returned"
                      : "incomplete",
                    {
                      interruption: toolInterruption(
                        outcomes.includes("faulted") ||
                          modes.includes("faulted"),
                        outcomes.includes("interrupted") ||
                          modes.includes("disconnected") ||
                          modes.includes("reconnected"),
                      ),
                    },
                  );
                },
              )
              .exhaustive();
          } catch (error) {
            if (error instanceof StationRefusedError)
              return result(item.call, error.detail, "incomplete");
            if (error instanceof LeaseLostError)
              return result(item.call, error.message, "incomplete", {
                interruption: "interrupted",
              });
            throw error;
          }
        });
        return outcome;
      }),
    );
    signal?.throwIfAborted();
    const results = outcomes.map((outcome, index) => {
      if (outcome.status === "fulfilled") return outcome.value;
      const call = calls[index];
      if (!call) throw new Error("Missing tool call for batch result");
      return result(
        call,
        `Tool call failed; its outcome may be unknown: ${String(outcome.reason)}`,
        "incomplete",
        { interruption: "interrupted" },
      );
    });
    if (formatter === defaultFormatter) return results;
    // WHY: Format after execution and error classification. A callback failure must
    // propagate without retrying a command or being treated as a robot failure.
    return results.map((result) => {
      const content = formatter(
        structuredClone({
          kind: "result",
          data: result.data,
          call: result.call,
          cameras: result.sequenceImages ? {} : result.cameras,
          sequenceImages: result.sequenceImages,
        }),
      );
      return {
        ...result,
        text: content.text,
        cameras: content.cameras,
        get modelData() {
          return parseModelData(this.text);
        },
      };
    });
  }

  /**
   * Wait until the arm holds again with no interlock of its own after a
   * disconnect or a recovery approval. False when the arm faulted first;
   * throws `LeaseLostError` when the lease is lost.
   */
  async function waitForArmRecovery(): Promise<boolean> {
    while (true) {
      if (tracking.isLost()) throw new LeaseLostError();
      const selected = armOf(await stationState());
      if (selected.mode === "faulted") return false;
      if (selected.mode === "holding" && armInterlocks(selected).length === 0)
        return true;
      await nextStateOrBackstop();
    }
  }

  /** Wake on the next state frame; the 1 s timer is only a liveness backstop. */
  async function nextStateOrBackstop(): Promise<void> {
    try {
      await connection.waitForState(() => true, {
        ...waitOptions,
        fresh: true,
        timeoutMilliseconds: 1000,
      });
    } catch (error) {
      signal?.throwIfAborted();
      if (!(error instanceof StationTimeoutError)) throw error;
    }
  }

  return {
    tools: catalog.tools.map((tool) => ({
      ...structuredClone(tool),
      description: options.descriptions?.[tool.name] ?? tool.description,
    })),
    observe,
    observeWhenReady,
    context,
    execute,
    waitForArmRecovery,
    /** Selected images from the latest sequence, retained for at most five minutes. */
    sequenceImages: () => tracking.sequenceImages.snapshot(),
    close: tracking.close,
  };
}

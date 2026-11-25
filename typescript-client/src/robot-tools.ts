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

class ObservationChanged extends Error {
  constructor(arm: string) {
    super(`Arm ${arm} changed during observation`);
  }
}

function result(
  call: RobotToolCall,
  value: unknown,
  isError = false,
  halt = false,
  interrupted = false,
  images: {
    cameras?: Record<string, Uint8Array>;
    sequenceImages?: SequenceImageHistory;
  } = {},
): RobotToolResult {
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
    isError,
    halt,
    interrupted,
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
    return result(
      call,
      answer,
      true,
      answer.reason === "faulted",
      ["stale_revision", "interlocked", "lease_held_elsewhere"].includes(
        answer.reason,
      ),
    );
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
    status !== "reached",
    outcomes.includes("faulted"),
    outcomes.includes("interrupted"),
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
    allowCameraInterlock = false,
    text: ObservationText = {},
  ): Promise<{
    output: Extract<RobotOutput, { kind: "observation" }>;
    state: StationState;
  }> {
    const capture = await connection.captureObservation(
      {
        arms: [],
        required_cameras: [...cameras],
        optional_cameras: [...optionalCameras],
      },
      signal,
    );
    switch (capture.status) {
      case "observation_required_camera_unavailable":
        throw new RequiredCameraUnavailable(capture.camera);
      case "observation_arm_disconnected":
        throw new Error(`Arm ${capture.arm} is disconnected`);
      case "observation_revision_changed":
        throw new ObservationChanged(capture.arm);
      case "observation_captured": {
        const state = apiSchemas.shape.StationState.parse(capture.state);
        requireNoInterlock(armOf(state), allowCameraInterlock);
        return {
          state,
          output: {
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
          },
        };
      }
    }
    throw new Error(
      `Unknown capture result ${JSON.stringify(capture satisfies never)}`,
    );
  }

  async function observe(
    allowCameraInterlock = false,
    text: ObservationText = {},
  ): Promise<RobotObservation> {
    const { output, state } = await captureObservation(
      allowCameraInterlock,
      text,
    );
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

  /**
   * Wait through arm recovery and required-camera interlocks, then observe
   * again. Needs the lease; throws `LeaseLostError` once it is lost, and
   * `ArmRecoveryEndedError` when the arm faults.
   */
  async function observeWhenReady(
    text: ObservationText = {},
  ): Promise<RobotObservation> {
    let cameraRetryMilliseconds = 100;
    while (true) {
      if (tracking.isLost()) throw new LeaseLostError();
      if (!lease.isHeld()) throw new LeaseLostError();
      const selected = armOf(await stationState());
      if (selected.mode === "faulted")
        throw new ArmRecoveryEndedError("Arm is faulted");
      if (
        armInterlocks(selected).length > 0 ||
        selected.mode === "disconnected" ||
        selected.mode === "reconnected" ||
        selected.mode === "disabled"
      ) {
        // WHY: Recovery shows up as a state frame; wake on the next one.
        await nextStateOrBackstop();
        continue;
      }
      try {
        await stationState(true);
        const observation = await observe(true, text);
        if (tracking.isLost()) throw new LeaseLostError();
        // WHY: Every required camera just gave a frame: clear its interlock.
        for (const interlock of armOf(await stationState()).interlocks)
          if (
            interlock.kind === "required_camera_unavailable" &&
            cameras.includes(interlock.camera)
          )
            await lease.reportCamera(interlock.camera, true);
        const after = armOf(await stationState(true));
        if (
          after.interlocks.length === 0 &&
          after.revision === observation.revisions[arm] &&
          !tracking.isLost()
        )
          return observation;
      } catch (error) {
        signal?.throwIfAborted();
        if (error instanceof RequiredCameraUnavailable)
          await lease.reportCamera(error.camera, false);
        else if (tracking.isLost()) throw new LeaseLostError();
        else if (!(error instanceof ObservationChanged)) {
          const latest = armOf(await stationState());
          if (latest.interlocks.length === 0 && latest.mode !== "faulted")
            throw error;
        }
      }
      await delay(cameraRetryMilliseconds);
      cameraRetryMilliseconds = Math.min(1000, cameraRetryMilliseconds * 2);
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
                result(call, error, true),
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
                      true,
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
                    false,
                    false,
                    false,
                    { sequenceImages: history },
                  );
                }
                const observation = await observe();
                return result(
                  item.call,
                  observation.data,
                  false,
                  false,
                  false,
                  {
                    cameras: observation.cameras,
                  },
                );
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
                  const sequenceId = await lease.startActionSequence(
                    capActionSequence(
                      message.action_sequence,
                      options.motionLimits,
                    ),
                    captureCameras,
                  );
                  if (captureCameras.length)
                    tracking.sequenceImages.select(sequenceId, call.id);
                  const sequence = await connection.waitForActionSequence(
                    sequenceId,
                    waitOptions,
                  );
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
                    sequence.phase.state !== "completed",
                    outcomes.includes("faulted") || modes.includes("faulted"),
                    outcomes.includes("interrupted") ||
                      modes.includes("disconnected") ||
                      modes.includes("reconnected"),
                  );
                },
              )
              .exhaustive();
          } catch (error) {
            if (error instanceof StationRefusedError)
              return result(item.call, error.detail, true);
            if (error instanceof LeaseLostError)
              return result(item.call, error.message, true, false, true);
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
        true,
        false,
        true,
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

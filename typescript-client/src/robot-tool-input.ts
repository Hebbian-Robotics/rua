import { match } from "ts-pattern";
import { z } from "zod";
import { apiSchemas } from "./generated/api-schemas.js";
import {
  armMotionCommands,
  armPowerCommands,
} from "./generated/arm-commands.js";
import { robotToolInputUnits } from "./generated/robot-tools.js";
import type { RobotToolCall } from "./robot-tool-types.js";
import type { StationCommand } from "./station-types.js";
import type {
  RobotToolName,
  SequenceAction,
  SequenceScope,
} from "./tool-catalog.js";

interface ToolInputContext {
  readonly arm: string;
  readonly sequenceScope: SequenceScope;
  readonly sequenceActions: ReadonlySet<SequenceAction>;
  readonly availableToolNames: ReadonlyMap<string, RobotToolName>;
  readonly revisions?: Readonly<Record<string, number>>;
}

type PreparedMessage = Extract<
  z.infer<typeof apiSchemas.shape.ClientMessage>,
  { kind: "command" | "move_arms" | "start_action_sequence" }
>;
type ParsedRobotToolCall =
  | {
      readonly kind: "message";
      readonly call: RobotToolCall;
      readonly message: PreparedMessage;
    }
  | {
      readonly kind: "preview";
      readonly call: RobotToolCall;
      readonly arm: string;
      readonly poses: readonly Omit<
        z.infer<typeof apiSchemas.shape.PosePreviewCandidate>,
        "id"
      >[];
    }
  | {
      readonly kind: "observe";
      readonly call: RobotToolCall;
      readonly wait: number;
      readonly source: "current" | "last_sequence";
    }
  | { readonly kind: "rehome"; readonly call: RobotToolCall }
  | {
      readonly kind: "invalid";
      readonly call: RobotToolCall;
      readonly error: string;
    };

const argumentsObject = z.record(z.string(), z.unknown());
const observeArguments = z
  .object({
    wait_seconds: z.number().min(0).max(10).default(0),
    source: z.enum(["current", "last_sequence"]).default("current"),
  })
  .strict();
const gripperArguments = z
  .object({
    arm: z.string(),
    opening: z.number(),
    max_joint_speed: z.number().nullish(),
  })
  .strict();
const action_sequenceArguments = z
  .object({ steps: z.array(z.unknown()).min(1) })
  .strict();
const stopArguments = z.object({}).strict();

function clientMessage(input: unknown) {
  return apiSchemas.shape.ClientMessage.parse(input);
}

/** The command message of one arm's command, planned at `expectedRevision`, without sending it. */
function commandMessageOf(arm: string) {
  const message = (command: StationCommand, expectedRevision?: number) =>
    clientMessage({
      kind: "command",
      request_id: null,
      arm,
      command,
      expected_revision: expectedRevision ?? null,
    });
  return { ...armMotionCommands(message), ...armPowerCommands(message) };
}

/** Convert explicit model units to the station's SI fields at the tool boundary. */
function convertModelArgumentsToStationUnits(value: unknown): unknown {
  if (Array.isArray(value))
    return value.map(convertModelArgumentsToStationUnits);
  if (typeof value !== "object" || value === null) return value;
  const converted: Record<string, unknown> = {};
  function scaleModelInputNumbers(value: unknown, factor: number): unknown {
    if (Array.isArray(value))
      return value.map((item) => scaleModelInputNumbers(item, factor));
    return typeof value === "number" ? value * factor : value;
  }
  for (const [key, item] of Object.entries(value)) {
    if (robotToolInputUnits.some((unit) => unit.station_field === key))
      throw new Error(`Use explicit model units for ${key}`);
    const unit = robotToolInputUnits.find((unit) => unit.model_field === key);
    if (unit !== undefined) {
      converted[unit.station_field] = scaleModelInputNumbers(
        item,
        unit.scale_to_station,
      );
    } else if (
      (key === "arms" || key === "goals") &&
      typeof item === "object" &&
      item !== null &&
      !Array.isArray(item)
    ) {
      converted[key] = Object.fromEntries(
        Object.entries(item).map(([arm, target]) => [
          arm,
          convertModelArgumentsToStationUnits(target),
        ]),
      );
    } else {
      converted[key] = convertModelArgumentsToStationUnits(item);
    }
  }
  return converted;
}

function addOrientationKind(value: unknown): unknown {
  if (typeof value !== "object" || value === null || Array.isArray(value))
    return value;
  if ("kind" in value) return value;
  if ("quaternion" in value) return { ...value, kind: "quaternion" };
  if ("approach" in value) return { ...value, kind: "grasp" };
  return value;
}

function addMovementOrientationKind(value: unknown): unknown {
  if (typeof value !== "object" || value === null || !("orientation" in value))
    return value;
  return { ...value, orientation: addOrientationKind(value.orientation) };
}

function addMovementKind(value: unknown): unknown {
  if (typeof value !== "object" || value === null || Array.isArray(value))
    return value;
  if ("kind" in value) return addMovementOrientationKind(value);
  const kind =
    "joints" in value ? "joints" : "position" in value ? "pose" : undefined;
  return addMovementOrientationKind(
    kind === undefined ? value : { ...value, kind },
  );
}

export function parseRobotToolCall(
  call: RobotToolCall,
  context: ToolInputContext,
): ParsedRobotToolCall {
  try {
    const args = argumentsObject.parse(
      convertModelArgumentsToStationUnits(call.arguments),
    );
    const toolName = context.availableToolNames.get(call.name);
    if (toolName === undefined)
      throw new Error(`Tool ${call.name} is not available`);
    return match(toolName)
      .returnType<ParsedRobotToolCall>()
      .with("action_sequence", () => {
        const input = action_sequenceArguments.parse(args);
        const message = clientMessage({
          kind: "start_action_sequence",
          request_id: null,
          action_sequence: {
            steps: input.steps.map((step) => {
              const armActions =
                context.sequenceScope.kind === "single"
                  ? { [context.arm]: step }
                  : z.object({ arms: argumentsObject }).strict().parse(step)
                      .arms;
              if (Object.keys(armActions).length === 0)
                throw new Error("A step needs at least one arm");
              return {
                arms: Object.fromEntries(
                  Object.entries(armActions).map(([name, rawAction]) => {
                    if (
                      context.sequenceScope.kind === "named" &&
                      !context.sequenceScope.arms.includes(name)
                    )
                      throw new Error(
                        `Arm ${name} is not available in this sequence`,
                      );
                    const raw = argumentsObject.parse(rawAction);
                    const action =
                      apiSchemas.shape.ActionSequenceArmAction.parse({
                        ...raw,
                        ...(Array.isArray(raw["waypoints"])
                          ? {
                              waypoints: raw["waypoints"].map(
                                (value: unknown) => {
                                  const point = argumentsObject.parse(value);
                                  return {
                                    ...point,
                                    at: addMovementKind(point["at"]),
                                  };
                                },
                              ),
                            }
                          : {}),
                        ...("goal" in raw
                          ? { goal: addMovementKind(raw["goal"]) }
                          : {}),
                      });
                    if (
                      ("goal" in action &&
                        (!context.sequenceActions.has(action.goal.kind) ||
                          action.waypoints?.some(
                            (point) =>
                              !context.sequenceActions.has(point.at.kind) ||
                              (point.gripper_opening != null &&
                                !context.sequenceActions.has("gripper")),
                          ))) ||
                      (action.gripper_opening != null &&
                        !context.sequenceActions.has("gripper"))
                    )
                      throw new Error(
                        "This action is not available in the configured sequence",
                      );
                    return [name, action];
                  }),
                ),
              };
            }),
            expected_revisions: context.revisions ?? {},
          },
        });
        if (message.kind !== "start_action_sequence")
          throw new Error("Invalid action_sequence");
        return { kind: "message", call, message };
      })
      .with("preview_pose", () => {
        const poses = z
          .array(
            z
              .object({
                pose: z.unknown(),
              })
              .strict(),
          )
          .min(1)
          .parse(args["poses"]);
        if (Object.keys(args).some((key) => key !== "poses"))
          throw new Error("Unexpected preview arguments");
        const request = apiSchemas.shape.PosePreviewBatchRequest.omit({
          poses: true,
        }).parse({ arm: context.arm });
        return {
          kind: "preview",
          call,
          arm: request.arm,
          poses: poses.map((candidate) =>
            apiSchemas.shape.PosePreviewCandidate.omit({ id: true }).parse({
              pose: addMovementOrientationKind(candidate.pose),
            }),
          ),
        };
      })
      .with("move_arms", () => {
        const message = clientMessage({
          kind: "move_arms",
          request_id: null,
          command: {
            ...args,
            goals:
              typeof args["goals"] === "object" && args["goals"] !== null
                ? Object.fromEntries(
                    Object.entries(args["goals"]).map(([name, target]) => [
                      name,
                      addMovementKind(target),
                    ]),
                  )
                : args["goals"],
          },
          expected_revisions: context.revisions ?? {},
        });
        if (message.kind !== "move_arms") throw new Error("Invalid group move");
        return { kind: "message", call, message };
      })
      .with("set_arm_gripper", () => {
        const input = gripperArguments.parse(args);
        const message = commandMessageOf(input.arm).setGripper(
          input.opening,
          input.max_joint_speed ?? undefined,
          context.revisions?.[input.arm],
        );
        if (message.kind !== "command")
          throw new Error("Invalid gripper command");
        return { kind: "message", call, message };
      })
      .with("stop", () => {
        stopArguments.parse(args);
        const message = commandMessageOf(context.arm).stop();
        if (message.kind !== "command") throw new Error("Invalid stop command");
        return { kind: "message", call, message };
      })
      .with("rehome", () => {
        stopArguments.parse(args);
        return { kind: "rehome", call };
      })
      .with("observe", () => {
        const input = observeArguments.parse(args);
        return {
          kind: "observe",
          call,
          wait: input.wait_seconds,
          source: input.source,
        };
      })
      .exhaustive();
  } catch (error) {
    return {
      kind: "invalid",
      call,
      error: `Invalid input: ${
        error instanceof z.ZodError ? z.prettifyError(error) : String(error)
      }`,
    };
  }
}

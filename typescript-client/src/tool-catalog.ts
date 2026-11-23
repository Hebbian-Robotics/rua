import { z } from "zod";
import { robotToolSpecifications } from "./generated/robot-tools.js";

export type SequenceAction = "pose" | "joints" | "gripper";
export type SequenceScope =
  | { readonly kind: "single" }
  | { readonly kind: "named"; readonly arms: readonly string[] };
export type RobotToolName =
  | "action_sequence"
  | "preview_pose"
  | "move_arms"
  | "set_arm_gripper"
  | "rehome"
  | "stop"
  | "observe";
export interface RobotToolCatalogConfiguration {
  readonly sequenceActions?: readonly SequenceAction[];
  readonly sequenceScope?: SequenceScope;
  readonly tools?: readonly RobotToolName[];
}

type JsonValue = string | number | boolean | null | JsonValue[] | JsonObject;
type JsonObject = { [key: string]: JsonValue };
const jsonObjectSchema = z.record(z.string(), z.json());
const defaultToolNames: readonly RobotToolName[] = [
  "action_sequence",
  "preview_pose",
  "stop",
  "observe",
];

function schemaObject(value: JsonValue | undefined): JsonObject {
  if (value === null || typeof value !== "object" || Array.isArray(value))
    throw new Error("Expected a generated schema object");
  return value;
}

/** The Rust catalog owns field definitions; composition selects their availability. */
export function createRobotToolCatalog(
  configuration: RobotToolCatalogConfiguration = {},
) {
  const defaults = configurationFromCapabilities(
    defaultRobotToolMapping(allRobotToolCapabilities()),
  );
  const sequenceActions = new Set(
    configuration.sequenceActions ?? defaults.sequenceActions ?? [],
  );
  if (
    [...sequenceActions].some(
      (action) => !["pose", "joints", "gripper"].includes(action),
    )
  )
    throw new Error("Unknown sequence action");
  const requestedScope = configuration.sequenceScope ??
    defaults.sequenceScope ?? { kind: "single" };
  const sequenceScope: SequenceScope =
    requestedScope.kind === "named"
      ? { kind: "named", arms: [...requestedScope.arms] }
      : { kind: "single" };
  const requestedToolNames = configuration.tools ?? defaults.tools;
  const toolNames = new Set(requestedToolNames);
  if (toolNames.size !== requestedToolNames.length)
    throw new Error("A tool appears more than once");
  if (toolNames.has("action_sequence") && sequenceActions.size === 0)
    throw new Error("An action sequence needs at least one available action");
  if (
    sequenceScope.kind === "named" &&
    (sequenceScope.arms.length === 0 ||
      sequenceScope.arms.some((arm) => arm.length === 0) ||
      new Set(sequenceScope.arms).size !== sequenceScope.arms.length)
  )
    throw new Error(
      "Named-arm sequences need a nonempty set of unique arm names",
    );
  const specificationsByName = new Map(
    robotToolSpecifications.map((tool) => [tool.name, tool]),
  );
  const tools = [...toolNames].map((name) => {
    const tool = specificationsByName.get(name);
    if (tool === undefined)
      throw new Error("Unknown tool in catalog configuration");
    const parameters = jsonObjectSchema.parse(tool.parameters);
    if (tool.name === "action_sequence") {
      const steps = schemaObject(
        schemaObject(parameters["properties"])["steps"],
      );
      const action = schemaObject(steps["items"]);
      const properties = schemaObject(action["properties"]);
      if (sequenceActions.has("pose") || sequenceActions.has("joints")) {
        const goal = schemaObject(properties["goal"]);
        const variants = Array.isArray(goal["oneOf"]) ? goal["oneOf"] : [goal];
        const available = variants.filter((variant) => {
          const movementProperties = schemaObject(
            schemaObject(variant)["properties"],
          );
          return sequenceActions.has(
            "joints_degrees" in movementProperties ? "joints" : "pose",
          );
        });
        properties["goal"] =
          available.length === 1 ? (available[0] ?? {}) : { oneOf: available };
      } else delete properties["goal"];
      if (!sequenceActions.has("gripper")) delete properties["gripper_opening"];
      const requirements = action["anyOf"];
      if (Array.isArray(requirements))
        action["anyOf"] = requirements.filter((requirement: unknown) => {
          if (
            requirement === null ||
            typeof requirement !== "object" ||
            !("required" in requirement)
          )
            return false;
          const required = requirement.required;
          return (
            Array.isArray(required) &&
            required.every(
              (name: unknown) => typeof name === "string" && name in properties,
            )
          );
        });
      if (sequenceScope.kind === "named") {
        steps["items"] = {
          type: "object",
          additionalProperties: false,
          required: ["arms"],
          properties: {
            arms: {
              type: "object",
              additionalProperties: false,
              minProperties: 1,
              properties: Object.fromEntries(
                sequenceScope.arms.map((arm) => [arm, action]),
              ),
            },
          },
        };
      }
    }
    return {
      ...tool,
      name,
      parameters,
    };
  });
  const capabilities = robotToolCapabilities({
    tools: [...toolNames],
    sequenceActions: [...sequenceActions],
    sequenceScope,
  });
  return { tools, sequenceActions, sequenceScope, capabilities };
}

export type RobotToolCapability =
  | {
      readonly name: "action_sequence";
      readonly scope: SequenceScope;
      readonly actions: readonly SequenceAction[];
    }
  | { readonly name: Exclude<RobotToolName, "action_sequence"> };
export type ToolTransformation = (
  capabilities: readonly RobotToolCapability[],
) => readonly RobotToolCapability[];

function robotToolCapabilities(
  configuration: RobotToolCatalogConfiguration = {},
): readonly RobotToolCapability[] {
  const scope: SequenceScope = configuration.sequenceScope ?? {
    kind: "single",
  };
  return (configuration.tools ?? defaultToolNames).map(
    (name): RobotToolCapability =>
      name === "action_sequence"
        ? {
            name,
            scope: structuredClone(scope),
            actions: [
              ...(configuration.sequenceActions ?? [
                "pose",
                "joints",
                "gripper",
              ]),
            ],
          }
        : { name },
  );
}

/** Start from the complete available set when replacing the default mapping. */
export function allRobotToolCapabilities(): readonly RobotToolCapability[] {
  return robotToolCapabilities({
    tools: robotToolSpecifications.map((tool) => tool.name as RobotToolName),
  });
}

export const defaultRobotToolMapping: ToolTransformation = (capabilities) =>
  capabilities.filter((tool) => defaultToolNames.includes(tool.name));

/** Filter typed sequence actions before deriving either schemas or runtime permissions. */
export function filterSequenceActions(
  keep: (action: SequenceAction) => boolean,
): ToolTransformation {
  return (capabilities) =>
    capabilities.map((tool) =>
      tool.name === "action_sequence"
        ? { ...tool, actions: tool.actions.filter(keep) }
        : tool,
    );
}

export function configurationFromCapabilities(
  capabilities: readonly RobotToolCapability[],
): Required<RobotToolCatalogConfiguration> {
  const sequence = capabilities.find((tool) => tool.name === "action_sequence");
  const names = capabilities.map((tool) => tool.name);
  if (new Set(names).size !== names.length)
    throw new Error("A tool appears more than once");
  return {
    tools: names,
    sequenceActions: sequence?.actions ?? [],
    sequenceScope: sequence?.scope ?? { kind: "single" },
  };
}

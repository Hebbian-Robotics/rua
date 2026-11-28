/** Readable model responses; station records retain their original SI values. */
import { apiSchemas } from "./generated/api-schemas.js";
import {
  modelOutcomes,
  modelOutputUnits,
  modelResponseFields,
} from "./generated/robot-tools.js";

const outputConversions = new Map<string, readonly [string, number]>(
  modelOutputUnits.map(([stationField, modelField, factor]) => [
    stationField,
    [modelField, factor],
  ]),
);

function rounded(value: number, decimals: number): number {
  const scale = 10 ** decimals;
  return (Math.sign(value) * Math.round(Math.abs(value) * scale)) / scale;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function arrayOrEmpty(value: unknown): readonly unknown[] {
  return Array.isArray(value) ? value : [];
}

function scaleNumbers(value: unknown, factor: number, round: boolean): unknown {
  if (Array.isArray(value))
    return value.map((item) => scaleNumbers(item, factor, round));
  if (typeof value !== "number") return value;
  const converted = value * factor;
  return round ? rounded(converted, 1) : converted;
}

function modelCommandRecord(
  record: Record<string, unknown>,
): Record<string, unknown> {
  if (!("command_id" in record)) return record;
  const parsed = apiSchemas.shape.CommandRecord.safeParse(record);
  if (!parsed.success) return record;
  const { command, phase, ...metadata } = parsed.data;
  const { kind, ...target } = command;
  const result: Record<string, unknown> = { ...metadata, ...target };
  if (command.kind === "policy_reach") {
    delete result["goal_pose"];
    result["goal_position"] = command.goal_pose.position;
    result["goal_quaternion"] = command.goal_pose.quaternion;
  }
  switch (phase.kind) {
    case "pending":
      break;
    case "finished": {
      result["completion"] = phase.completion;
      result["achieved"] = phase.achieved;
      if (command.kind === "move") {
        const rule = modelOutcomes.commands[phase.completion.outcome];
        result["goal_status"] =
          command.pose_limited || command.path_limited
            ? rule.limited_target
            : rule.target;
      }
      break;
    }
    case "not_started":
    case "outcome_unknown":
      result["completion"] = {
        outcome:
          phase.kind === "outcome_unknown" ? "interrupted" : phase.outcome,
        detail: phase.detail,
        finished_unix_nano: phase.finished_unix_nano,
      };
      if (phase.kind === "not_started")
        result[kind === "move" ? "goal_status" : "phase"] = "not_started";
      break;
    default:
      throw new Error(`Unknown phase ${JSON.stringify(phase satisfies never)}`);
  }
  return result;
}

/** Convert model-facing JSON, keeping copyable orientations at full precision. */
export function modelResponse(value: unknown): unknown {
  return formatModelField(value, "");
}

function formatModelField(value: unknown, fieldName: string): unknown {
  if (fieldName === "approach") return value;
  if (Array.isArray(value))
    return value.map((item) => formatModelField(item, fieldName));
  if (isRecord(value)) {
    const formatted: [string, unknown][] = [];
    const record = modelCommandRecord(value);
    for (const [key, item] of Object.entries(record)) {
      if (
        item == null ||
        (key === "checkpoints" && Array.isArray(item) && item.length === 0)
      )
        continue;
      const conversion = outputConversions.get(key);
      if (conversion) {
        const [modelField, conversionFactor] = conversion;
        formatted.push([
          modelField,
          scaleNumbers(
            item,
            conversionFactor,
            modelField !== "jaw_yaw_degrees",
          ),
        ]);
      } else {
        formatted.push([key, formatModelField(item, key)]);
      }
    }
    return Object.fromEntries(formatted);
  }
  if (typeof value === "number" && !Number.isInteger(value)) {
    const timestamp =
      fieldName.includes("unix_seconds") || fieldName === "sampled_at";
    return rounded(value, timestamp ? 3 : 4);
  }
  return value;
}

const diagnosticFields = new Set<string>(modelResponseFields.diagnostic);

export interface ConciseOptions {
  /**
   * Keep a station state's `description` (frames and conventions) and `lease` holder.
   * Observations leave them out because the model is given the conventions separately.
   */
  keepStationContext?: boolean;
}

function keepFields(
  record: Record<string, unknown>,
  kept: readonly string[],
): Record<string, unknown> {
  return Object.fromEntries(
    kept.flatMap((key) => (key in record ? [[key, record[key]]] : [])),
  );
}

// INVARIANT: Earlier steps retain their command outcome and any limitation; the last step
// keeps full concise detail for the model's next decision.
function compactEarlierStep(value: unknown): unknown {
  if (!Array.isArray(value)) return conciseOutput(value);
  return value.map((entry: unknown) => {
    if (!isRecord(entry)) return conciseOutput(entry);
    const record = entry["record"];
    if (!isRecord(record)) return conciseOutput(entry);
    return conciseOutput({
      ...keepFields(entry, ["arm"]),
      record: keepFields(
        modelCommandRecord(record),
        modelResponseFields.earlier_step,
      ),
    });
  });
}

function withWaypointReversalHint(phase: unknown): unknown {
  if (!isRecord(phase) || !isRecord(phase["failure"])) return phase;
  const failure = phase["failure"];
  const detail = failure["detail"];
  // WHY: The station reports this as text, so match the known error exactly.
  if (
    failure["stage"] !== "preparation" ||
    failure["reason"] !== "invalid_target" ||
    typeof detail !== "string" ||
    !/^Waypoint \d+ requires a stop or reversal; use a separate step$/.test(
      detail,
    )
  )
    return phase;
  return {
    ...phase,
    failure: {
      ...failure,
      hint: "Waypoint index is zero-based. Split this move into two steps at that waypoint; each step can still have waypoints.",
    },
  };
}

/**
 * Keep outcomes and actionable state; omit history and rendering geometry. The shared
 * specification is `tests/fixtures/model-concise.json`, also run by Python and Rust.
 */
export function conciseOutput(
  value: unknown,
  options: ConciseOptions = {},
): unknown {
  if (Array.isArray(value))
    return value.map((item) => conciseOutput(item, options));
  if (!isRecord(value)) return value;
  const record = modelCommandRecord(value);
  if ("arms" in record && "joint_names" in record) {
    const arms = arrayOrEmpty(record["arms"]);
    return {
      ...keepFields(record, [
        ...modelResponseFields.state,
        ...(options.keepStationContext
          ? modelResponseFields.station_context
          : []),
      ]),
      arms: arms.map((arm) =>
        conciseOutput(
          isRecord(arm) ? keepFields(arm, modelResponseFields.arm) : {},
        ),
      ),
    };
  }
  if ("requested_steps" in record && "steps" in record) {
    const requested = arrayOrEmpty(record["requested_steps"]);
    const steps = arrayOrEmpty(record["steps"]);
    return {
      ...keepFields(record, ["arms"]),
      ...(record["phase"] === undefined
        ? {}
        : { phase: withWaypointReversalHint(record["phase"]) }),
      requested_step_count: requested.length,
      recorded_step_count: steps.length,
      steps: steps.map((step, index) =>
        index === steps.length - 1
          ? conciseOutput(step)
          : compactEarlierStep(step),
      ),
    };
  }
  return Object.fromEntries(
    Object.entries(record)
      .filter(([key]) => !diagnosticFields.has(key))
      .map(([key, item]) => [key, conciseOutput(item, options)]),
  );
}

/** The model view: the concise layer, then readable units and rounding. */
export function conciseResponse(
  value: unknown,
  options: ConciseOptions = {},
): unknown {
  return modelResponse(conciseOutput(value, options));
}

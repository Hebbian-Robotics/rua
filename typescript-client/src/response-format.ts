/** Readable model responses; station records retain their original SI values. */
import {
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

function scaleNumbers(value: unknown, factor: number): unknown {
  if (Array.isArray(value))
    return value.map((item) => scaleNumbers(item, factor));
  return typeof value === "number" ? rounded(value * factor, 1) : value;
}

function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function modelCommandRecord(
  record: Record<string, unknown>,
): Record<string, unknown> {
  const { command, phase, ...metadata } = record;
  if (
    !isObject(command) ||
    !isObject(phase) ||
    typeof record["command_id"] !== "string"
  )
    return record;
  const { kind, target_pose: pose, ...target } = command;
  const phaseKind = phase["kind"];
  if (
    typeof kind !== "string" ||
    !["pending", "not_started", "finished", "outcome_unknown"].some(
      (known) => known === phaseKind,
    )
  )
    return record;
  const result: Record<string, unknown> = { ...metadata, ...target };
  if (isObject(pose)) {
    result["target_position"] = pose["position"];
    result["target_quaternion"] = pose["quaternion"];
  }
  if (phaseKind === "finished") {
    result["completion"] = phase["completion"];
    result["achieved"] = phase["achieved"];
    const completion = phase["completion"];
    if (kind === "move" && isObject(completion)) {
      const outcome = completion["outcome"];
      if (outcome === "completed") {
        const limited =
          command["pose_limited"] != null || command["path_limited"] != null;
        result["target_status"] = limited ? "limited" : "reached";
      } else {
        result["target_status"] =
          outcome === "stopped_short" ? "stopped_short" : "partial";
      }
    }
  } else if (phaseKind === "not_started" || phaseKind === "outcome_unknown") {
    result["completion"] = {
      outcome:
        phaseKind === "outcome_unknown" ? "interrupted" : phase["outcome"],
      detail: phase["detail"],
      finished_at: phase["finished_at"],
    };
    if (phaseKind === "not_started") {
      if (kind === "move") result["target_status"] = "not_started";
      else result["phase"] = "not_started";
    }
  }
  return result;
}

/** Convert model-facing JSON, keeping copyable orientations at full precision. */
export function modelResponse(value: unknown): unknown {
  return formatModelField(value, "");
}

function formatModelField(value: unknown, fieldName: string): unknown {
  if (["approach", "jaw_yaw_degrees"].includes(fieldName)) return value;
  if (Array.isArray(value))
    return value.map((item) => formatModelField(item, fieldName));
  if (typeof value === "object" && value !== null) {
    const formatted: Record<string, unknown> = {};
    const record = modelCommandRecord(value as Record<string, unknown>);
    for (const [key, item] of Object.entries(record)) {
      if (
        item == null ||
        (key === "checkpoints" && Array.isArray(item) && item.length === 0)
      )
        continue;
      const conversion = outputConversions.get(key);
      if (conversion)
        formatted[conversion[0]] = scaleNumbers(item, conversion[1]);
      else formatted[key] = formatModelField(item, key);
    }
    return formatted;
  }
  if (typeof value === "number" && !Number.isInteger(value)) {
    const timestamp =
      fieldName.includes("unix_seconds") ||
      ["accepted_at", "sampled_at", "finished_at"].includes(fieldName);
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

// Earlier steps retain their command outcome and any limitation; the last step
// keeps full concise detail for the model's next decision.
function compactEarlierStep(value: unknown): unknown {
  if (!Array.isArray(value)) return conciseOutput(value);
  return value.map((item) => {
    if (typeof item !== "object" || item === null || !("record" in item))
      return conciseOutput(item);
    const entry = item as Record<string, unknown>;
    const record = entry["record"];
    if (typeof record !== "object" || record === null)
      return conciseOutput(item);
    return conciseOutput({
      ...keepFields(entry, ["arm"]),
      record: keepFields(
        modelCommandRecord(record as Record<string, unknown>),
        modelResponseFields.earlier_step,
      ),
    });
  });
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
  if (typeof value !== "object" || value === null) return value;
  const record = modelCommandRecord(value as Record<string, unknown>);
  if ("arms" in record && "joint_names" in record) {
    const arms = Array.isArray(record["arms"]) ? record["arms"] : [];
    return {
      ...keepFields(record, [
        ...modelResponseFields.state,
        ...(options.keepStationContext
          ? modelResponseFields.station_context
          : []),
      ]),
      arms: arms.map((arm) =>
        conciseOutput(
          keepFields(arm as Record<string, unknown>, modelResponseFields.arm),
        ),
      ),
    };
  }
  if ("requested_steps" in record && "steps" in record) {
    const requested = record["requested_steps"] as unknown[];
    const steps = record["steps"] as unknown[];
    return {
      ...keepFields(record, ["arms", "phase"]),
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

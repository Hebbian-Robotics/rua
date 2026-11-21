/** Readable model responses; station records retain their original SI values. */
const positionFields: Readonly<Record<string, string>> = {
  position: "position_mm",
  target_position: "target_position_mm",
  tool_position: "tool_position_mm",
  surface_height_meters: "surface_height_mm",
  leeway_meters: "leeway_mm",
  model_points: "model_points_mm",
  position_error_meters: "position_error_mm",
  position_shortfall_meters: "position_shortfall_mm",
};

const angleFields: Readonly<Record<string, string>> = {
  joints: "joints_degrees",
  measured_joints: "measured_joints_degrees",
  measured_joint_velocities: "measured_joint_velocities_degrees_per_second",
  commanded_joints: "commanded_joints_degrees",
  rest_pose: "rest_pose_degrees",
  target_joints: "target_joints_degrees",
  goal_joints: "goal_joints_degrees",
  joint_error_radians: "joint_error_degrees",
  joint_errors_radians: "joint_errors_degrees",
  orientation_error_radians: "orientation_error_degrees",
  orientation_shortfall_radians: "orientation_shortfall_degrees",
  joint_speed: "joint_speed_degrees_per_second",
  max_joint_speed: "max_joint_speed_degrees_per_second",
};

function rounded(value: number, decimals: number): number {
  const scale = 10 ** decimals;
  return (Math.sign(value) * Math.round(Math.abs(value) * scale)) / scale;
}

function scaleNumbers(value: unknown, factor: number): unknown {
  if (Array.isArray(value))
    return value.map((item) => scaleNumbers(item, factor));
  return typeof value === "number" ? rounded(value * factor, 1) : value;
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
    const record = value as Record<string, unknown>;
    for (const [key, item] of Object.entries(value)) {
      if (
        item == null ||
        (key === "checkpoints" && Array.isArray(item) && item.length === 0)
      )
        continue;
      if (
        key === "target_status" &&
        item === "reached" &&
        (record["pose_limited"] != null || record["path_limited"] != null)
      ) {
        formatted[key] = "limited";
        continue;
      }
      const positionField = positionFields[key];
      const angleField = angleFields[key];
      if (positionField !== undefined)
        formatted[positionField] = scaleNumbers(item, 1000);
      else if (angleField !== undefined)
        formatted[angleField] = scaleNumbers(item, 180 / Math.PI);
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

const stateFields = [
  "robot",
  "joint_names",
  "max_joint_speed",
  "surface_guard",
];
const stationContextFields = ["description", "lease"];
const armFields = [
  "arm",
  "mode",
  "status",
  "fault",
  "active_trip",
  "interlocks",
  "recovery_issue",
  "revision",
  "sampled_at_unix_seconds",
  "measured_joints",
  "tool",
  "gripper_opening",
];
const diagnosticFields = [
  "id",
  "command_id",
  "group_id",
  "sequence_id",
  "action_sequence_id",
  "checkpoints",
  "trace_context",
  "model_points",
  "accepted_at",
  "finished_at",
];

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
      record: keepFields(record as Record<string, unknown>, [
        "label",
        "completion",
        "target_status",
        "pose_limited",
        "path_limited",
      ]),
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
  const record = value as Record<string, unknown>;
  if ("arms" in record && "joint_names" in record) {
    const arms = Array.isArray(record["arms"]) ? record["arms"] : [];
    return {
      ...keepFields(record, [
        ...stateFields,
        ...(options.keepStationContext ? stationContextFields : []),
      ]),
      arms: arms.map((arm) =>
        conciseOutput(keepFields(arm as Record<string, unknown>, armFields)),
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
      .filter(([key]) => !diagnosticFields.includes(key))
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

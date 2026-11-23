// Names for the station contract types the viewer uses. The
// types and the zod schemas come from the Rust types (`uv run rua-dev contracts`);
// nothing here restates a field.

import type * as z from "zod";
import { type ApiSchemas, apiSchemas } from "./generated/api-schemas.js";

/** What the viewer sends: fields with defaults may be left out. */
export type ClientMessage = z.input<typeof apiSchemas>["ClientMessage"];
export type ServerMessage = ApiSchemas["ServerMessage"];
export type StationState = ApiSchemas["StationState"];
export type ArmState = StationState["arms"][number];
export type LiveRun = ApiSchemas["LiveRun"];
export type Interlock = ArmState["interlocks"][number];

export type StationCommand = Extract<
  ClientMessage,
  { kind: "command" }
>["command"];
export type MoveArmsCommand = Extract<
  ClientMessage,
  { kind: "move_arms" }
>["command"];
export type PreviewPose = Extract<ClientMessage, { kind: "preview" }>["pose"];
export type ActionSequenceRequest = Extract<
  ClientMessage,
  { kind: "start_action_sequence" }
>["action_sequence"];
export type ActionSequenceRecord = ApiSchemas["ActionSequenceRecord"];
export type PoseOrientation = NonNullable<PreviewPose["orientation"]>;

export type CommandAnswer = Extract<
  ServerMessage,
  { kind: "command_answer" }
>["answer"];
export type PreviewAnswer = Extract<
  ServerMessage,
  { kind: "preview_answer" }
>["answer"];
export type LeaseAnswer = Extract<ServerMessage, { kind: "lease" }>["answer"];
export type PoseProjection = Extract<
  PreviewAnswer,
  { status: "projected" }
>["projection"];
/** A command's record, as the state and the answers carry it. */
export type CommandRecord = NonNullable<ArmState["last_command"]>;
export type LeaseStatus = NonNullable<StationState["lease"]>;
export type LeaseHolder = LeaseStatus["holder"];
export type OnInterruption = LeaseStatus["on_interruption"];

export const serverMessageSchema = apiSchemas.shape.ServerMessage;

/** The caller's W3C trace context, which parents a command's span on the station. */
export type TraceContext = NonNullable<
  Extract<ClientMessage, { kind: "command" }>["trace_context"]
>;

/** The public outcome of a terminal command, including an interrupted controller. */
export function commandResult(record: CommandRecord | null | undefined):
  | {
      outcome: ApiSchemas["CommandOutcome"];
      finished_at: number;
      detail?: string | null;
    }
  | undefined {
  const phase = record?.phase;
  if (!phase) return undefined;
  switch (phase.kind) {
    case "pending":
      return undefined;
    case "finished":
      return phase.completion;
    case "not_started":
      return phase;
    case "outcome_unknown":
      return {
        outcome: "interrupted",
        finished_at: phase.finished_at,
        detail: phase.detail,
      };
  }
  throw new Error(
    `Unknown command phase ${JSON.stringify(phase satisfies never)}`,
  );
}

/** The admitted path and its limits, when this command has a planned move. */
export function commandPlan(record: CommandRecord | null | undefined) {
  return record?.command.kind === "move" ? record.command : undefined;
}

/** Whether a planned move was adjusted before reaching the requested target. */
export function commandWasLimited(
  record: CommandRecord | null | undefined,
): boolean {
  const plan = commandPlan(record);
  return plan?.pose_limited != null || plan?.path_limited != null;
}

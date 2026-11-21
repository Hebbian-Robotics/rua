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

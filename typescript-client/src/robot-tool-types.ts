import type { StationState, TraceContext } from "./station-types.js";

export interface RobotToolCall {
  id: string;
  name: string;
  arguments: unknown;
}

export interface RobotToolResult {
  call: RobotToolCall;
  text: string;
  /** Complete payload in original station units and precision. */
  data: unknown;
  /** Selected model text parsed as JSON when possible. */
  readonly modelData: unknown;
  isError: boolean;
  halt: boolean;
  interrupted: boolean;
  /** Images explicitly requested by an observe call, labelled for model messages. */
  cameras?: Record<string, Uint8Array>;
  sequenceImages?: SequenceImageHistory;
}

export interface RobotObservation {
  /** Complete station state in original units and precision. */
  data: StationState;
  text: string;
  cameras: Record<string, Uint8Array>;
  /** Each arm's revision when observed; commands carry it as `expected_revision`. */
  revisions: Record<string, number>;
}

/** What an observation's text carries above the robot's state: extra lines, then a note. */
export interface ObservationText {
  note?: string;
  extraLines?: readonly string[];
}

/**
 * Runs one tool call. `run` takes the W3C trace context its station commands
 * should be parented to; a loop uses this to put each call in a span.
 */
export type ToolCallWrapper = (
  call: RobotToolCall,
  run: (traceContext?: TraceContext) => Promise<RobotToolResult>,
) => Promise<RobotToolResult>;

/** A camera sample retrieved during a model action sequence; timing is client-local. */
export interface SequenceImage {
  camera: string;
  jpeg: Uint8Array;
  retrievedBetweenUnixSeconds: readonly [number, number];
  elapsedSeconds: number;
  boundary: "start" | "motion" | "step" | "end";
}

/** Bounded visual evidence, independent of station recording and motion outcomes. */
export interface SequenceImageHistory {
  callId: string;
  images: readonly SequenceImage[];
  missedSamples: number;
  truncated: boolean;
}

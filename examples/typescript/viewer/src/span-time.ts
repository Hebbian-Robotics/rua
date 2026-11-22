import type { TraceSpan } from "./recording-timeline.js";

export type { TraceSpan };

const NANOSECONDS_PER_SECOND = 1_000_000_000;

export function spanStart(span: TraceSpan): number {
  return Number(span.start_time_unix_nano) / NANOSECONDS_PER_SECOND;
}

export function spanEnd(span: TraceSpan): number {
  return Number(span.end_time_unix_nano) / NANOSECONDS_PER_SECOND;
}

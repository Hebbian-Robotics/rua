// Report spans at start and finish so the viewer can show work in progress.
// A small sink suffices here: IDs, parents, attributes and W3C trace context.

import { randomBytes } from "node:crypto";
import type {
  ClientMessage,
  StationConnection,
  TraceContext,
} from "@hebbian/robot-use-client";
import { StationConnectionError } from "@hebbian/robot-use-client";

export const KIND_ATTRIBUTE = "hebbian.kind";

export type AttributeValue = string | number | boolean | string[];
export type Attributes = Record<string, AttributeValue>;
export type WireSpan = Required<
  Extract<ClientMessage, { kind: "run_spans" }>["spans"][number]
>;

export type SpanSink = (spans: WireSpan[]) => void;

export type UnixNanosecondsClock = () => bigint;
const systemClock: UnixNanosecondsClock = () => BigInt(Date.now()) * 1_000_000n;

export function stationSpanSink(connection: StationConnection): SpanSink {
  return (spans) => {
    try {
      connection.recordRunSpans(spans);
    } catch (error) {
      if (!(error instanceof StationConnectionError)) throw error;
    }
  };
}

export class Span {
  private readonly attributes: Attributes;
  private status: WireSpan["status"] = {
    code: "STATUS_CODE_UNSET",
    message: null,
  };
  private readonly startNanoseconds: bigint;
  readonly spanId = randomBytes(8).toString("hex");

  constructor(
    private readonly tracer: Tracer,
    readonly name: string,
    attributes: Attributes,
    readonly traceId: string,
    private readonly parentSpanId: string | null,
  ) {
    this.attributes = { ...attributes };
    this.startNanoseconds = tracer.clock();
    tracer.sink([
      this.wire({ ...this.attributes, "hebbian.active": true }, tracer.clock()),
    ]);
  }

  setAttribute(key: string, value: AttributeValue): void {
    this.attributes[key] = value;
  }

  setAttributes(attributes: Attributes): void {
    Object.assign(this.attributes, attributes);
  }

  setError(message: string): void {
    this.status = { code: "STATUS_CODE_ERROR", message };
  }

  traceContext(): TraceContext {
    return {
      traceparent: `00-${this.traceId}-${this.spanId}-01`,
      tracestate: null,
    };
  }

  end(): void {
    this.tracer.sink([this.wire(this.attributes, this.tracer.clock())]);
  }

  private wire(attributes: Attributes, endNanoseconds: bigint): WireSpan {
    return {
      trace_id: this.traceId,
      span_id: this.spanId,
      parent_span_id: this.parentSpanId,
      name: this.name,
      start_time_unix_nano: this.startNanoseconds.toString(),
      end_time_unix_nano: endNanoseconds.toString(),
      attributes,
      status: this.status,
    };
  }
}

export class Tracer {
  private readonly traceId = randomBytes(16).toString("hex");
  private root: Span | null = null;

  constructor(
    readonly sink: SpanSink,
    readonly clock: UnixNanosecondsClock = systemClock,
  ) {}

  async span<Result>(
    name: string,
    attributes: Attributes,
    work: (span: Span) => Promise<Result>,
  ): Promise<Result> {
    const span = new Span(
      this,
      name,
      attributes,
      this.traceId,
      this.root?.spanId ?? null,
    );
    this.root ??= span;
    try {
      return await work(span);
    } catch (error) {
      span.setError(error instanceof Error ? error.message : String(error));
      throw error;
    } finally {
      span.end();
    }
  }
}

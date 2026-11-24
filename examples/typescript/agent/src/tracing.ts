import {
  type Attributes,
  type Context,
  context,
  defaultTextMapSetter,
  type HrTime,
  ROOT_CONTEXT,
  type Span,
  SpanStatusCode,
  type Tracer,
  trace,
} from "@opentelemetry/api";
import { AsyncLocalStorageContextManager } from "@opentelemetry/context-async-hooks";
import { W3CTraceContextPropagator } from "@opentelemetry/core";
import {
  type ReadableSpan,
  type SpanProcessor,
  TracerProvider,
} from "@opentelemetry/sdk-trace";
import type {
  ClientMessage,
  StationConnection,
  TraceContext,
} from "hebbian-rua";
import { StationConnectionError } from "hebbian-rua";

export const KIND_ATTRIBUTE = "hebbian.kind";

const asyncContext = new AsyncLocalStorageContextManager().enable();
if (!context.setGlobalContextManager(asyncContext)) asyncContext.disable();

export type WireSpan = Required<
  Extract<ClientMessage, { kind: "run_spans" }>["spans"][number]
>;

const wireStatusCodes = {
  [SpanStatusCode.UNSET]: "STATUS_CODE_UNSET",
  [SpanStatusCode.OK]: "STATUS_CODE_OK",
  [SpanStatusCode.ERROR]: "STATUS_CODE_ERROR",
} as const;

function unixNanoseconds([seconds, nanoseconds]: HrTime): string {
  return (BigInt(seconds) * 1_000_000_000n + BigInt(nanoseconds)).toString();
}

function stationSpanProcessor(connection: StationConnection): SpanProcessor {
  function reportSpan(span: ReadableSpan, active: boolean): void {
    const { spanId, traceId } = span.spanContext();
    const start = unixNanoseconds(span.startTime);

    try {
      connection.recordRunSpans([
        {
          trace_id: traceId,
          span_id: spanId,
          parent_span_id: span.parentSpanContext?.spanId ?? null,
          name: span.name,
          start_time_unix_nano: start,
          end_time_unix_nano: active ? start : unixNanoseconds(span.endTime),
          attributes: active
            ? { ...span.attributes, "hebbian.active": true }
            : span.attributes,
          status: {
            code: wireStatusCodes[span.status.code],
            message: span.status.message ?? null,
          },
        },
      ]);
    } catch (error) {
      if (!(error instanceof StationConnectionError)) throw error;
    }
  }

  return {
    // The viewer uses the start update to show work while the span is open.
    onStart: (span) => reportSpan(span, true),
    onEnd: (span) => reportSpan(span, false),
    forceFlush: () => Promise.resolve(),
    shutdown: () => Promise.resolve(),
  };
}

const traceContextPropagator = new W3CTraceContextPropagator();

export function traceContextForSpan(span: Span): TraceContext | undefined {
  const headers: Record<string, string> = {};
  traceContextPropagator.inject(
    trace.setSpan(ROOT_CONTEXT, span),
    headers,
    defaultTextMapSetter,
  );
  const { traceparent, tracestate } = headers;
  return traceparent === undefined
    ? undefined
    : { traceparent, tracestate: tracestate ?? null };
}

export function markSpanError(span: Span, message: string): void {
  span.setStatus({ code: SpanStatusCode.ERROR, message });
}

function spanRunner(tracer: Tracer, parent: Context) {
  return async function withSpan<Result>(
    name: string,
    attributes: Attributes,
    work: (span: Span) => Promise<Result>,
  ): Promise<Result> {
    const span = tracer.startSpan(name, { attributes }, parent);
    try {
      return await context.with(
        trace.setSpan(parent, span),
        work,
        undefined,
        span,
      );
    } catch (error) {
      markSpanError(
        span,
        error instanceof Error ? error.message : String(error),
      );
      throw error;
    } finally {
      span.end();
    }
  };
}

export type SpanRunner = ReturnType<typeof spanRunner>;

export async function withStationTrace<Result>(
  connection: StationConnection,
  name: string,
  attributes: Attributes,
  work: (
    runSpan: Span,
    withChildSpan: SpanRunner,
    tracerProvider: TracerProvider,
  ) => Promise<Result>,
): Promise<Result> {
  const provider = new TracerProvider({
    spanProcessors: [stationSpanProcessor(connection)],
  });
  const tracer = provider.getTracer("hebbian.agent");
  const withRootSpan = spanRunner(tracer, ROOT_CONTEXT);
  try {
    return await withRootSpan(name, attributes, (runSpan) =>
      work(
        runSpan,
        spanRunner(tracer, trace.setSpan(ROOT_CONTEXT, runSpan)),
        provider,
      ),
    );
  } finally {
    await provider.shutdown();
  }
}

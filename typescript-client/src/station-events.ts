import { Effect } from "effect";
import { createParser } from "eventsource-parser";
import { z } from "zod";

import { type ServerMessage, serverMessageSchema } from "./station-types.js";

/** One decoded server-sent event. */
export interface StationEvent {
  kind: string;
  data: string;
  id: string | null;
}

const stationMessageEnvelope = z.object({ kind: z.string() });
const snapshotEnvelope = z.object({
  station_instance_id: z.string().optional(),
  run_sequence: z.int().nonnegative(),
  items: z.array(z.unknown()),
});
const replayEnvelope = z.array(z.unknown());
const stationMessagesByKind = new Map<
  string,
  (typeof serverMessageSchema.options)[number]
>(
  serverMessageSchema.options.map((schema) => [
    schema.shape.kind.value,
    schema,
  ]),
);

export type IncomingStationMessage =
  | { kind: "known"; message: ServerMessage }
  | { kind: "resync_required" }
  | { kind: "ignored_frame" }
  | { kind: "unknown"; wireKind: string };

function parseStationMessage(payload: unknown): IncomingStationMessage {
  const { kind } = stationMessageEnvelope.parse(payload);
  if (kind === "resync_required") return { kind };
  // WHY: Arm frames are consumed by the Rust SDK. This SDK uses station state.
  if (kind === "frame") return { kind: "ignored_frame" };
  const schema = stationMessagesByKind.get(kind);
  if (schema === undefined) return { kind: "unknown", wireKind: kind };
  return { kind: "known", message: schema.parse(payload) };
}

export function parseStationEvent(event: StationEvent) {
  switch (event.kind) {
    case "resync_required":
      return { kind: "resync_required" } as const;
    case "run":
    case "station":
      return parseStationMessage(JSON.parse(event.data));
    case "snapshot": {
      const snapshot = snapshotEnvelope.parse(JSON.parse(event.data));
      return {
        kind: "snapshot",
        runSequence: snapshot.run_sequence,
        stationInstanceId: snapshot.station_instance_id,
        messages: snapshot.items.map(parseStationMessage),
      } as const;
    }
    default:
      return { kind: "ignored_event", wireKind: event.kind } as const;
  }
}

export function parseRunReplay(payload: unknown): IncomingStationMessage[] {
  return replayEnvelope.parse(payload).map(parseStationMessage);
}

/** Read events in order, awaiting application handling before reading more bytes. */
export function readStationEvents(
  response: Response,
  handleEvent: (event: StationEvent) => Promise<void>,
  byteReadTimeoutMilliseconds?: number,
): Effect.Effect<void, unknown> {
  if (!response.body)
    return Effect.fail(new Error("Station event stream has no body"));
  const body = response.body;
  const decoder = new TextDecoder();
  let cursor: string | null = null;
  const pending: StationEvent[] = [];
  const parser = createParser({
    onId: (id) => {
      cursor = id;
    },
    onEvent: (event) => {
      pending.push({
        kind: event.event || "message",
        data: event.data,
        id: cursor,
      });
    },
  });
  return Effect.acquireUseRelease(
    Effect.try({ try: () => body.getReader(), catch: (error) => error }),
    (reader) =>
      Effect.gen(function* () {
        while (true) {
          const read = Effect.tryPromise({
            try: () => reader.read(),
            catch: (error) => error,
          });
          const chunk = yield* byteReadTimeoutMilliseconds === undefined
            ? read
            : read.pipe(
                Effect.timeoutOrElse({
                  duration: byteReadTimeoutMilliseconds,
                  orElse: () =>
                    Effect.fail(new Error("Station event stream went silent")),
                }),
              );
          if (chunk.done) break;
          yield* Effect.try({
            try: () =>
              parser.feed(decoder.decode(chunk.value, { stream: true })),
            catch: (error) => error,
          });
          for (const event of pending) {
            yield* Effect.tryPromise({
              try: () => handleEvent(event),
              catch: (error) => error,
            });
          }
          pending.length = 0;
        }
      }),
    (reader) =>
      Effect.tryPromise({
        try: async () => {
          try {
            await reader.cancel();
          } finally {
            reader.releaseLock();
          }
        },
        catch: (error) => error,
      }).pipe(Effect.ignore),
  );
}

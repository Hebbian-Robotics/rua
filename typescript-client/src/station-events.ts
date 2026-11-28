import { Effect, Stream } from "effect";
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
export function readStationEvents<Requirements>(
  bytes: Stream.Stream<Uint8Array, unknown>,
  handleEvent: (
    event: StationEvent,
  ) => Effect.Effect<void, unknown, Requirements>,
  byteReadTimeoutMilliseconds?: number,
): Effect.Effect<void, unknown, Requirements> {
  return Effect.suspend(() => {
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
    const timedBytes =
      byteReadTimeoutMilliseconds === undefined
        ? bytes
        : bytes.pipe(
            Stream.timeoutOrElse({
              duration: byteReadTimeoutMilliseconds,
              orElse: () =>
                Stream.fail(new Error("Station event stream went silent")),
            }),
          );
    return timedBytes.pipe(
      Stream.decodeText,
      Stream.runForEach((text) =>
        Effect.gen(function* () {
          yield* Effect.try({
            try: () => parser.feed(text),
            catch: (error) => error,
          });
          for (const event of pending) {
            yield* handleEvent(event);
          }
          pending.length = 0;
        }),
      ),
    );
  });
}

import { createParser } from "eventsource-parser";

/** One decoded server-sent event. */
export interface StationEvent {
  kind: string;
  data: string;
  id: string | null;
}

/** Read events in order, awaiting application handling before reading more bytes. */
export async function readStationEvents(
  response: Response,
  handleEvent: (event: StationEvent) => Promise<void>,
  byteReadTimeout?: { milliseconds: number; abort: () => void },
): Promise<void> {
  if (!response.body) throw new Error("Station event stream has no body");
  const reader = response.body.getReader();
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
  try {
    while (true) {
      const timeout = byteReadTimeout
        ? setTimeout(byteReadTimeout.abort, byteReadTimeout.milliseconds)
        : undefined;
      const chunk = await reader.read().finally(() => clearTimeout(timeout));
      if (chunk.done) break;
      parser.feed(decoder.decode(chunk.value, { stream: true }));
      for (const event of pending) await handleEvent(event);
      pending.length = 0;
    }
  } finally {
    reader.releaseLock();
  }
}

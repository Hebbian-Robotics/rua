import { expect, test } from "vitest";
import { readStationEvents, type StationEvent } from "../src/station-events.js";

test("preserves the station cursor when mapping events for application handlers", async () => {
  const bytes = new TextEncoder().encode(
    ': keepalive\r\nid: station:7\r\nevent: station\r\ndata: {"label":\r\ndata: "café"}\r\n\r\nevent: resync_required\ndata: fell behind\n\n',
  );
  const body = new ReadableStream<Uint8Array>({
    start(controller) {
      controller.enqueue(bytes);
      controller.close();
    },
  });
  const events: StationEvent[] = [];
  await readStationEvents(new Response(body), async (event) => {
    events.push(event);
  });
  expect(events).toEqual([
    { kind: "station", id: "station:7", data: '{"label":\n"café"}' },
    { kind: "resync_required", id: "station:7", data: "fell behind" },
  ]);
});

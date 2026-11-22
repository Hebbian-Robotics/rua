import { readFileSync } from "node:fs";
import type { IReadable } from "@mcap/core";
import { expect, test } from "vitest";
import {
  loadRecording,
  type RecordingTimeline,
  recordingBounds,
} from "../src/recording-timeline.js";

// Written by the Rust recorder's MCAP writer (zstd chunks): one arm sample,
// an empty one, a command, an observation, two span updates and a bad sample.
const take = new Uint8Array(
  readFileSync(new URL("fixtures/take.mcap", import.meta.url)),
);

function bytesReadable(bytes: Uint8Array): IReadable {
  return {
    size: async () => BigInt(bytes.length),
    read: async (offset, length) =>
      bytes.slice(Number(offset), Number(offset + length)),
  };
}

test("builds aligned arm tracks, events and spans from a recorder-written MCAP", async () => {
  const { timeline } = await loadRecording(bytesReadable(take));
  expect(timeline.truncated).toBe(true);
  expect(timeline.state?.times).toEqual([1, 2]);
  const [arm] = timeline.state?.arms ?? [];
  expect(timeline.state?.arms).toHaveLength(1);
  expect(arm?.measuredJoints).toEqual([[0.2, null]]);
  expect(arm?.commandedJoints).toEqual([[0.5, null]]);
  expect(arm?.destinationJoints).toEqual([[0.8], null]);
  expect(arm?.gripperOpening).toEqual([0.4, null]);
  expect(timeline.events).toHaveLength(3);
  expect(timeline.events[1]?.data).toMatchObject({
    event: { cameras: ["front"] },
  });
  expect(timeline.events.at(-1)?.topic).toBe("/station/recording_errors");
  expect(timeline.spans).toHaveLength(1);
  expect(timeline.spans[0]?.end_time_unix_nano).toBe("1300000000");
  expect(timeline.spans[0]?.attributes).not.toHaveProperty("hebbian.active");
});

test("a recording cut off mid-write keeps what was read before the cut", async () => {
  const { timeline } = await loadRecording(
    bytesReadable(take.slice(0, take.length - 40)),
  );
  expect(timeline.truncated).toBe(true);
});

test("playback bounds exclude replayed events and support many camera segments", () => {
  const timeline: RecordingTimeline = {
    clips: [],
    events: [],
    spans: [],
    state: null,
    truncated: false,
  };
  expect(recordingBounds(timeline)).toEqual({ start: 0, end: 0 });
  timeline.events = [
    { timestamp: 1, topic: "/station/command_records", data: {} },
    { timestamp: 10, topic: "/recording/metadata", data: {} },
    { timestamp: 20, topic: "/station/command_records", data: {} },
  ];
  expect(recordingBounds(timeline)).toEqual({ start: 10, end: 20 });
  timeline.clips = Array.from({ length: 200_000 }, (_, index) => ({
    cameraName: "front",
    segment: String(index),
    startTime: index + 10,
    durationSeconds: 2,
    startNanoseconds: 0n,
    endNanoseconds: 2_000_000_000n,
    width: 640,
    height: 480,
  }));
  expect(recordingBounds(timeline)).toEqual({ start: 10, end: 200_011 });
});

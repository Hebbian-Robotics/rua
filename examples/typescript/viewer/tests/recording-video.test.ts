import { readFileSync } from "node:fs";
import type { IReadable } from "@mcap/core";
import {
  ALL_FORMATS,
  BufferSource,
  EncodedPacketSink,
  Input,
} from "mediabunny";
import { expect, test } from "vitest";
import {
  groupRecordingClips,
  loadRecording,
} from "../src/recording-timeline.js";

// CONSTRAINT: Written by the recorder's production writer: camera "chest", three segments
// of 3, 1 and 1 frames at irregular times, the second one rotated.
const bytes = new Uint8Array(
  readFileSync(new URL("fixtures/with-video.mcap", import.meta.url)),
);
const readable: IReadable = {
  size: async () => BigInt(bytes.length),
  read: async (offset, length) =>
    bytes.slice(Number(offset), Number(offset + length)),
};

test("segment markers become clips with their capture times and picture sizes", async () => {
  const { timeline } = await loadRecording(readable);
  expect(
    timeline.clips.map((clip) => [
      clip.cameraName,
      clip.width,
      clip.height,
      Math.round(clip.durationSeconds * 1e6),
    ]),
  ).toEqual([
    ["chest", 16, 8, 175_111],
    ["chest", 8, 16, 0],
    ["chest", 32, 16, 0],
  ]);
  expect(timeline.clips[0]?.startTime).toBeCloseTo(1_700_000_000.123456, 5);
});

test("a clip's packets become an MP4 with the recorded timestamps", async () => {
  const recording = await loadRecording(readable);
  const clip = recording.timeline.clips[0];
  if (!clip) throw new Error("no clip");
  const mp4 = await recording.clipVideo(clip);
  const input = new Input({
    source: new BufferSource(mp4),
    formats: ALL_FORMATS,
  });
  const track = await input.getPrimaryVideoTrack();
  if (!track) throw new Error("no video track");
  expect(track.codec).toBe("avc");
  const timestamps: number[] = [];
  for await (const packet of new EncodedPacketSink(track).packets())
    timestamps.push(packet.timestamp);
  // CONSTRAINT: The MP4 clock ticks in whole units of its timescale, so allow a few microseconds.
  const expected = [0, 66_667, 175_111];
  expect(timestamps).toHaveLength(3);
  for (const [index, time] of timestamps.entries())
    expect(Math.abs(time * 1e6 - (expected[index] ?? 0))).toBeLessThan(20);
});

test("successive segments are grouped into one camera without discarding frames", async () => {
  const { timeline } = await loadRecording(readable);
  const cameras = groupRecordingClips(timeline.clips);
  expect([...cameras.keys()]).toEqual(["chest"]);
  expect(cameras.get("chest")).toEqual(timeline.clips);
});

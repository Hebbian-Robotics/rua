// Builds what the recordings page shows from a recording's `recording.mcap`.
// The station only lists and serves the files; the page downloads the MCAP and
// reads its JSON channels here, the same way any other MCAP reader would.

import {
  compressedVideoData,
  remuxToMp4,
} from "@hebbian/mcap-tools/compressed-video";
import {
  type IReadable,
  McapIndexedReader,
  McapStreamReader,
} from "@mcap/core";
import { decompress } from "fzstd";
import { apiSchemas } from "hebbian-rua";
import * as z from "zod";

const MAXIMUM_EVENTS = 5000;
const NANOSECONDS_PER_SECOND = 1_000_000_000;
const EVENT_TOPICS = new Set([
  "/recording/metadata",
  "/station/requests",
  "/station/command_records",
  "/station/rejections",
  "/station/recording_errors",
  "/station/recording_statistics",
  "/model/events",
]);

const recordedSpanSchema = z
  .object({
    traceId: z.string(),
    spanId: z.string(),
    parentSpanId: z.string().nullish(),
    name: z.string(),
    startTimeUnixNano: apiSchemas.shape.UnixNanos,
    endTimeUnixNano: apiSchemas.shape.UnixNanos,
    attributes: z.record(z.string(), z.unknown()).default({}),
    status: apiSchemas.shape.TraceSpan.shape.status,
  })
  .transform(
    (span): TraceSpan => ({
      trace_id: span.traceId,
      span_id: span.spanId,
      parent_span_id: span.parentSpanId ?? null,
      name: span.name,
      start_time_unix_nano: span.startTimeUnixNano,
      end_time_unix_nano: span.endTimeUnixNano,
      attributes: span.attributes,
      status: span.status,
    }),
  );
export type TraceSpan = z.infer<typeof apiSchemas.shape.TraceSpan>;

const armSampleSchema = z.object({
  arm: z.string(),
  base_pose: apiSchemas.shape.ArmState.shape.base_pose,
  measured_joints: z.array(z.number()),
  commanded_joints: z.array(z.number()).nullish(),
  gripper_opening: z.number().nullish(),
  active_command: z
    .object({
      target_joints: z.array(z.number()).nullish(),
      path_limited: z.unknown().nullish(),
      pose_limited: z.unknown().nullish(),
    })
    .nullish(),
});
const stationSampleSchema = z.object({
  joint_names: z.array(z.string()),
  arms: z.array(armSampleSchema),
});
type ArmSample = z.infer<typeof armSampleSchema>;

interface ArmTrack {
  arm: string;
  basePose: ArmSample["base_pose"];

  measuredJoints: (number | null)[][];
  commandedJoints: (number | null)[][];
  destinationJoints: (number[] | null)[];
  destinationLimited: boolean[];
  gripperOpening: (number | null)[];
}

export interface StateTrack {
  times: number[];
  jointNames: string[];
  arms: ArmTrack[];
}

interface TimelineEvent {
  timestamp: number;
  topic: string;
  data: Record<string, unknown>;
}

export interface RecordingClip {
  cameraName: string;

  segment: string;
  startTime: number;
  durationSeconds: number;
  startNanoseconds: bigint;
  endNanoseconds: bigint;
  width: number;
  height: number;
}

export function groupRecordingClips(
  clips: readonly RecordingClip[],
): Map<string, RecordingClip[]> {
  const cameras = new Map<string, RecordingClip[]>();
  for (const clip of clips) {
    const cameraClips = cameras.get(clip.cameraName) ?? [];
    cameraClips.push(clip);
    cameras.set(clip.cameraName, cameraClips);
  }
  return cameras;
}

export interface RecordingTimeline {
  clips: RecordingClip[];
  events: TimelineEvent[];

  truncated: boolean;
  state: StateTrack | null;
  spans: TraceSpan[];
}

function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

const modelEventSchema = z.looseObject({
  event: z.looseObject({ type: z.string(), cameras: z.unknown() }).optional(),
});

function eventData(topic: string, value: unknown): Record<string, unknown> {
  if (!isObject(value)) throw new Error("expected a JSON object");
  if (topic !== "/model/events") return value;
  const { event, ...rest } = modelEventSchema.parse(value);
  if (event?.type !== "observation") return value;
  // Observations embed whole camera images; keep only which cameras were seen.
  const cameras = isObject(event.cameras) ? Object.keys(event.cameras) : [];
  return { ...rest, event: { ...event, cameras } };
}

function armTrack(
  arm: string,
  snapshots: (ArmSample | null)[],
  jointCount: number,
): ArmTrack {
  const joints = (
    pick: (snapshot: ArmSample) => number[] | null | undefined,
  ): (number | null)[][] =>
    Array.from({ length: jointCount }, (_, index) =>
      snapshots.map((snapshot) => {
        const values = snapshot ? pick(snapshot) : null;
        return values?.[index] ?? null;
      }),
    );
  return {
    arm,
    basePose:
      snapshots.find((snapshot) => snapshot !== null)?.base_pose ?? null,
    measuredJoints: joints((snapshot) => snapshot.measured_joints),
    commandedJoints: joints((snapshot) => snapshot.commanded_joints),
    destinationJoints: snapshots.map((snapshot) => {
      const target = snapshot?.active_command?.target_joints;
      return target && target.length === jointCount ? target : null;
    }),
    destinationLimited: snapshots.map(
      (snapshot) =>
        snapshot?.active_command?.path_limited != null ||
        snapshot?.active_command?.pose_limited != null,
    ),
    gripperOpening: snapshots.map(
      (snapshot) => snapshot?.gripper_opening ?? null,
    ),
  };
}

function stateTrack(
  samples: { time: number; jointNames: string[]; arms: ArmSample[] }[],
): StateTrack | null {
  if (samples.length === 0) return null;
  samples.sort((first, second) => first.time - second.time);
  const jointNames =
    samples.findLast((sample) => sample.jointNames.length > 0)?.jointNames ??
    [];
  const snapshotsByArm = new Map<string, (ArmSample | null)[]>();
  for (const [index, sample] of samples.entries()) {
    for (const snapshots of snapshotsByArm.values()) snapshots.push(null);
    for (const arm of sample.arms) {
      let snapshots = snapshotsByArm.get(arm.arm);
      if (!snapshots) {
        snapshots = new Array<ArmSample | null>(index + 1).fill(null);
        snapshotsByArm.set(arm.arm, snapshots);
      }
      snapshots[index] = arm;
    }
  }
  return {
    times: samples.map((sample) => sample.time),
    jointNames,
    arms: [...snapshotsByArm.entries()]
      .sort(([first], [second]) => first.localeCompare(second))
      .map(([arm, snapshots]) => armTrack(arm, snapshots, jointNames.length)),
  };
}

const segmentTopic = /^\/cameras\/([^/]+)\/segments$/;
const videoTopic = (camera: string) => `/cameras/${camera}/video`;
const isVideoTopic = (topic: string) => topic.endsWith("/video");

const segmentMarkerSchema = z.object({
  camera: z.string(),
  segment: z.string(),
  // Strings: a nanosecond Unix time does not fit a JSON number exactly.
  started_unix_nano: z.string().optional(),
  ended_unix_nano: z.string().optional(),
  width: z.number().optional(),
  height: z.number().optional(),
});

interface TopicMessage {
  topic: string;
  logTime: bigint;
  data: Uint8Array;
}

interface MessageSource {
  telemetry(): AsyncIterable<TopicMessage>;
  video(
    camera: string,
    startNanoseconds: bigint,
    endNanoseconds: bigint,
  ): AsyncIterable<TopicMessage>;
}

const decompressHandlers = {
  zstd: (buffer: Uint8Array, decompressedSize: bigint) =>
    decompress(buffer, new Uint8Array(Number(decompressedSize))),
};

// WHY: Use the finished recording's index to read only chunks for the requested
// telemetry or camera video.
async function indexedSource(readable: IReadable): Promise<MessageSource> {
  const reader = await McapIndexedReader.Initialize({
    readable,
    decompressHandlers,
  });
  const topicOf = (channelId: number) =>
    reader.channelsById.get(channelId)?.topic ?? "";
  async function* messages(
    topics: string[],
    startTime?: bigint,
    endTime?: bigint,
  ): AsyncGenerator<TopicMessage> {
    if (topics.length === 0) return;
    for await (const message of reader.readMessages({
      topics,
      startTime,
      endTime,
    }))
      yield {
        topic: topicOf(message.channelId),
        logTime: message.logTime,
        data: message.data,
      };
  }
  return {
    telemetry: () =>
      messages(
        [...reader.channelsById.values()]
          .map((channel) => channel.topic)
          .filter((topic) => !isVideoTopic(topic)),
      ),
    video: (camera, startNanoseconds, endNanoseconds) =>
      messages([videoTopic(camera)], startNanoseconds, endNanoseconds + 1n),
  };
}

// WHY: A truncated recording may lack an index; scan the complete download to retain
// all messages before the cut.
function streamedSource(bytes: Uint8Array): {
  source: MessageSource;
  truncated: boolean;
} {
  const reader = new McapStreamReader({ decompressHandlers });
  reader.append(bytes);
  const topics = new Map<number, string>();
  const all: TopicMessage[] = [];
  let truncated = false;
  try {
    for (
      let record = reader.nextRecord();
      record;
      record = reader.nextRecord()
    ) {
      if (record.type === "Channel") topics.set(record.id, record.topic);
      else if (record.type === "Message")
        all.push({
          topic: topics.get(record.channelId) ?? "",
          logTime: record.logTime,
          data: record.data,
        });
    }
  } catch {
    truncated = true;
  }
  if (!reader.done()) truncated = true;
  return {
    truncated,
    source: {
      telemetry: async function* () {
        for (const message of all)
          if (!isVideoTopic(message.topic)) yield message;
      },
      video: async function* (camera, startNanoseconds, endNanoseconds) {
        for (const message of all)
          if (
            message.topic === videoTopic(camera) &&
            message.logTime >= startNanoseconds &&
            message.logTime <= endNanoseconds
          )
            yield message;
      },
    },
  };
}

async function buildTimeline(
  messages: AsyncIterable<TopicMessage>,
  truncatedInput: boolean,
): Promise<RecordingTimeline> {
  const events: TimelineEvent[] = [];
  const spans = new Map<string, TraceSpan>();
  const samples: { time: number; jointNames: string[]; arms: ArmSample[] }[] =
    [];
  const clips = new Map<string, RecordingClip>();
  let truncated = truncatedInput;

  function addEvent(
    time: number,
    topic: string,
    data: Record<string, unknown>,
  ) {
    if (events.length < MAXIMUM_EVENTS)
      events.push({ timestamp: time, topic, data });
    else truncated = true;
  }

  const decoder = new TextDecoder();
  for await (const message of messages) {
    const { topic } = message;
    const time = Number(message.logTime) / NANOSECONDS_PER_SECOND;
    const isSegment = segmentTopic.test(topic);
    if (
      !isSegment &&
      topic !== "/station/state" &&
      topic !== "/trace/spans" &&
      !EVENT_TOPICS.has(topic)
    )
      continue;
    try {
      const value: unknown = JSON.parse(decoder.decode(message.data));
      if (isSegment) {
        const marker = segmentMarkerSchema.parse(value);
        const clip = clips.get(marker.segment) ?? {
          cameraName: marker.camera,
          segment: marker.segment,
          startTime: time,
          durationSeconds: 0,
          startNanoseconds: message.logTime,
          endNanoseconds: message.logTime,
          width: marker.width ?? 0,
          height: marker.height ?? 0,
        };
        if (marker.started_unix_nano !== undefined) {
          clip.startNanoseconds = BigInt(marker.started_unix_nano);
          clip.startTime =
            Number(clip.startNanoseconds) / NANOSECONDS_PER_SECOND;
          clip.width = marker.width ?? clip.width;
          clip.height = marker.height ?? clip.height;
        }
        if (marker.ended_unix_nano !== undefined) {
          clip.endNanoseconds = BigInt(marker.ended_unix_nano);
          clip.durationSeconds =
            Number(clip.endNanoseconds - clip.startNanoseconds) /
            NANOSECONDS_PER_SECOND;
        }
        clips.set(marker.segment, clip);
      } else if (topic === "/station/state") {
        const sample = stationSampleSchema.parse(value);
        samples.push({
          time,
          jointNames: sample.joint_names,
          arms: sample.arms,
        });
      } else if (topic === "/trace/spans") {
        const span = recordedSpanSchema.parse(value);
        spans.set(`${span.trace_id}/${span.span_id}`, span);
      } else {
        addEvent(time, topic, eventData(topic, value));
      }
    } catch (error: unknown) {
      truncated = true;
      addEvent(time, "/station/recording_errors", {
        source: "playback",
        topic,
        detail: `Invalid recording data: ${error instanceof Error ? error.message : String(error)}; sample skipped`,
      });
    }
  }

  events.sort((first, second) => first.timestamp - second.timestamp);
  return {
    clips: [...clips.values()].sort(
      (first, second) => first.startTime - second.startTime,
    ),
    events,
    truncated,
    state: stateTrack(samples),
    spans: [...spans.values()].sort((first, second) =>
      Number(
        BigInt(first.start_time_unix_nano) -
          BigInt(second.start_time_unix_nano),
      ),
    ),
  };
}

export interface LoadedRecording {
  timeline: RecordingTimeline;

  clipVideo(clip: RecordingClip): Promise<Uint8Array>;
}

export async function loadRecording(
  readable: IReadable,
): Promise<LoadedRecording> {
  let source: MessageSource;
  let truncated = false;
  try {
    source = await indexedSource(readable);
  } catch {
    const size = await readable.size();
    const streamed = streamedSource(await readable.read(0n, size));
    source = streamed.source;
    truncated = streamed.truncated;
  }
  const timeline = await buildTimeline(source.telemetry(), truncated);
  return {
    timeline,
    async clipVideo(clip) {
      const packets = [];
      for await (const message of source.video(
        clip.cameraName,
        clip.startNanoseconds,
        clip.endNanoseconds,
      ))
        packets.push({
          logTimeNanoseconds: message.logTime,
          data: compressedVideoData(message.data),
        });
      packets.sort((first, second) =>
        Number(first.logTimeNanoseconds - second.logTimeNanoseconds),
      );
      return remuxToMp4(packets, clip.width, clip.height);
    },
  };
}

export function recordingBounds(timeline: RecordingTimeline): {
  start: number;
  end: number;
} {
  // The station snapshot can replay command records from before recording began.
  // Metadata is written at recorder startup, so those records cannot extend playback.
  const recordingStartedAt = timeline.events.find(
    (event) => event.topic === "/recording/metadata",
  )?.timestamp;
  const values = [
    ...timeline.clips.flatMap((clip) => [
      clip.startTime,
      clip.startTime + clip.durationSeconds,
    ]),
    ...timeline.events
      .filter(
        (event) =>
          recordingStartedAt === undefined ||
          event.timestamp >= recordingStartedAt,
      )
      .map((event) => event.timestamp),
    ...(timeline.state?.times.length
      ? [timeline.state.times[0] ?? 0, timeline.state.times.at(-1) ?? 0]
      : []),
  ];
  if (values.length === 0) return { start: 0, end: 0 };
  let start = Number.POSITIVE_INFINITY;
  let end = Number.NEGATIVE_INFINITY;
  for (const timestamp of values) {
    start = Math.min(start, timestamp);
    end = Math.max(end, timestamp);
  }
  return { start, end };
}

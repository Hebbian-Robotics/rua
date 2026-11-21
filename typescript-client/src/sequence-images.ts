import type {
  SequenceImage,
  SequenceImageHistory,
} from "./robot-tool-types.js";

const MAXIMUM_BYTES = 16 * 1024 * 1024;
const MAXIMUM_IMAGES = 64;
const MAXIMUM_MODEL_BYTES = 2 * 1024 * 1024;
const MAXIMUM_MODEL_IMAGES = 8;
const RETENTION_MILLISECONDS = 5 * 60 * 1000;

/** Retain the first sample and recent motion under both image-count and byte limits. */
export class SequenceImages {
  private history: SequenceImageHistory | undefined;
  private bytes = 0;
  private expiresAt = 0;
  private expiry: ReturnType<typeof setTimeout> | undefined;

  begin(callId: string): SequenceImageHistory {
    this.clear();
    this.history = { callId, images: [], missedSamples: 0, truncated: false };
    this.bytes = 0;
    this.expiresAt = Number.POSITIVE_INFINITY;
    return this.history;
  }

  private refreshExpiry(): void {
    this.expiresAt = performance.now() + RETENTION_MILLISECONDS;
    clearTimeout(this.expiry);
    this.expiry = setTimeout(() => this.clear(), RETENTION_MILLISECONDS);
    this.expiry.unref?.();
  }

  add(history: SequenceImageHistory, image: SequenceImage): void {
    if (this.history !== history) return;
    if (image.jpeg.byteLength > MAXIMUM_MODEL_BYTES) {
      history.missedSamples++;
      return;
    }
    const images = [...history.images, image];
    this.bytes += image.jpeg.byteLength;
    while (images.length > MAXIMUM_IMAGES || this.bytes > MAXIMUM_BYTES) {
      const removed = images.splice(1, 1)[0];
      if (removed) this.bytes -= removed.jpeg.byteLength;
      history.truncated = true;
    }
    history.images = images;
  }

  finish(history: SequenceImageHistory): void {
    if (this.history === history) {
      this.refreshExpiry();
    }
  }

  snapshot(): SequenceImageHistory | undefined {
    if (performance.now() >= this.expiresAt) {
      this.clear();
      return undefined;
    }
    if (!this.history) return undefined;
    const images = this.history.images;
    const selected: SequenceImage[] = [];
    let bytes = 0;
    const count = Math.min(MAXIMUM_MODEL_IMAGES, images.length);
    for (let index = count - 1; index >= 0; index--) {
      const image =
        images[
          Math.round((index * (images.length - 1)) / Math.max(1, count - 1))
        ];
      if (image && bytes + image.jpeg.byteLength <= MAXIMUM_MODEL_BYTES) {
        selected.push(image);
        bytes += image.jpeg.byteLength;
      }
    }
    return structuredClone({ ...this.history, images: selected.reverse() });
  }

  clear(): void {
    clearTimeout(this.expiry);
    this.expiry = undefined;
    if (this.history) this.history.images = [];
    this.history = undefined;
    this.bytes = 0;
  }
}

/** Sample separately from command execution; never wait for a camera before starting motion. */
export async function captureSequenceImages<Result>(
  historyStore: SequenceImages,
  callId: string,
  cameras: readonly string[],
  mediaUrl: string,
  subscribeSteps: (sample: () => void) => () => void,
  run: () => Promise<Result>,
): Promise<Result> {
  if (cameras.length === 0) {
    historyStore.clear();
    return run();
  }
  const history = historyStore.begin(callId);
  const startedAt = performance.now();
  let activeCapture: Promise<void> | undefined;
  let controller: AbortController | undefined;
  let cameraOffset = 0;
  async function capture(boundary: SequenceImage["boundary"]): Promise<void> {
    controller = new AbortController();
    const signal = AbortSignal.any([
      controller.signal,
      AbortSignal.timeout(500),
    ]);
    // Sequential reads bound the number of in-flight JPEGs regardless of camera count.
    const orderedCameras = [
      ...cameras.slice(cameraOffset),
      ...cameras.slice(0, cameraOffset),
    ];
    cameraOffset = (cameraOffset + 1) % cameras.length;
    for (const camera of orderedCameras) {
      if (signal.aborted) {
        history.missedSamples++;
        continue;
      }
      const before = Date.now() / 1000;
      try {
        const response = await fetch(
          new URL(`/cameras/${encodeURIComponent(camera)}`, mediaUrl),
          { signal, priority: "low" },
        );
        if (!response.ok || !response.body)
          throw new Error("Snapshot unavailable");
        const jpeg = await readBoundedJpeg(response.body);
        historyStore.add(history, {
          camera,
          jpeg,
          boundary,
          retrievedBetweenUnixSeconds: [before, Date.now() / 1000],
          elapsedSeconds: (performance.now() - startedAt) / 1000,
        });
      } catch {
        history.missedSamples++;
      }
    }
  }
  function sample(boundary: SequenceImage["boundary"]): void {
    if (activeCapture) return;
    activeCapture = capture(boundary).finally(() => {
      activeCapture = undefined;
    });
  }
  sample("start");
  const timer = setInterval(() => sample("motion"), 250);
  const unsubscribe = subscribeSteps(() => sample("step"));
  try {
    return await run();
  } finally {
    clearInterval(timer);
    unsubscribe();
    controller?.abort();
    try {
      await activeCapture;
      await capture("end");
    } finally {
      historyStore.finish(history);
    }
  }
}

async function readBoundedJpeg(
  body: ReadableStream<Uint8Array>,
): Promise<Uint8Array> {
  const reader = body.getReader();
  const chunks: Uint8Array[] = [];
  let size = 0;
  try {
    while (true) {
      const { value, done } = await reader.read();
      if (done) break;
      size += value.byteLength;
      if (size > MAXIMUM_MODEL_BYTES) throw new Error("Snapshot too large");
      chunks.push(value);
    }
  } finally {
    await reader.cancel();
  }
  const jpeg = new Uint8Array(size);
  let offset = 0;
  for (const chunk of chunks) {
    jpeg.set(chunk, offset);
    offset += chunk.byteLength;
  }
  return jpeg;
}

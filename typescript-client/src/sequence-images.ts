import type { ApiSchemas } from "./generated/api-schemas.js";
import type { SequenceImageHistory } from "./robot-tool-types.js";

export class SequenceImages {
  private latest?: {
    id: string;
    callId: string;
    cached?: {
      history: SequenceImageHistory;
      expiresAt: number;
      expiry: ReturnType<typeof setTimeout>;
    };
  };

  clear(): void {
    clearTimeout(this.latest?.cached?.expiry);
    delete this.latest;
  }

  select(id: string, callId: string): void {
    this.clear();
    this.latest = { id, callId };
  }

  execution(): string | undefined {
    return this.latest?.id;
  }

  cache(id: string, history: ApiSchemas["SequenceCameraHistory"]): void {
    const latest = this.latest;
    if (latest?.id !== id) return;
    const decoded: SequenceImageHistory = {
      callId: latest.callId,
      missedSamples: history.missed_samples,
      truncated: history.truncated,
      images: history.images.map((image) => {
        const [started, finished] = image.frame.retrieved_between_unix_seconds;
        if (started === undefined || finished === undefined)
          throw new Error("Sequence image is missing retrieval times");
        return {
          camera: image.camera,
          jpeg: Uint8Array.from(atob(image.frame.jpeg_base64), (byte) =>
            byte.charCodeAt(0),
          ),
          retrievedBetweenUnixSeconds: [started, finished],
          elapsedSeconds: image.elapsed_seconds,
          boundary: image.boundary,
        };
      }),
    };
    const remaining =
      Math.min(300, Math.max(0, history.expires_in_seconds)) * 1000;
    clearTimeout(latest.cached?.expiry);
    const expiry = setTimeout(() => {
      delete latest.cached;
    }, remaining);
    if (typeof expiry === "object" && "unref" in expiry) expiry.unref();
    latest.cached = {
      expiresAt: performance.now() + remaining,
      expiry,
      history: decoded,
    };
  }

  snapshot(): SequenceImageHistory | undefined {
    if (
      this.latest?.cached &&
      this.latest.cached.expiresAt <= performance.now()
    ) {
      clearTimeout(this.latest.cached.expiry);
      delete this.latest.cached;
    }
    return structuredClone(this.latest?.cached?.history);
  }
}

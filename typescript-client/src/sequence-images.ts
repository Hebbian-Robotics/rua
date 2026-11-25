import type { ApiSchemas } from "./generated/api-schemas.js";
import type { SequenceImageHistory } from "./robot-tool-types.js";

export class SequenceImages {
  private latest?: {
    id: string;
    callId: string;
    cached?: { history: SequenceImageHistory; expiresAt: number };
  };

  clear(): void {
    delete this.latest;
  }

  select(id: string, callId: string): void {
    this.latest = { id, callId };
  }

  execution(): string | undefined {
    return this.latest?.id;
  }

  cache(id: string, history: ApiSchemas["SequenceCameraHistory"]): void {
    if (this.latest?.id !== id) return;
    this.latest.cached = {
      expiresAt: performance.now() + history.expires_in_seconds * 1000,
      history: {
        callId: this.latest.callId,
        missedSamples: history.missed_samples,
        truncated: history.truncated,
        images: history.images.map((image) => {
          const [started, finished] =
            image.frame.retrieved_between_unix_seconds;
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
      },
    };
  }

  snapshot(): SequenceImageHistory | undefined {
    if (
      this.latest?.cached &&
      this.latest.cached.expiresAt <= performance.now()
    )
      delete this.latest.cached;
    return this.latest?.cached?.history;
  }
}

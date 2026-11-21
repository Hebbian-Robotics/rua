// The media service's cameras, phone invites and the executor's traces, over the
// generated routes.

import { z } from "zod";
import type { apiSchemas } from "./generated/api-schemas.js";
import { createStationApi } from "./generated/http-routes.js";
import { CameraUnavailableError, StationHttpError } from "./station-errors.js";
import { stationRequest } from "./station-request.js";

export const DEFAULT_STATION_URL = "http://127.0.0.1:8792";
export const DEFAULT_MEDIA_URL = "http://127.0.0.1:8781";

const CAPTURED_AT_HEADER = "X-Captured-At-Unix-Ns";

export type PhoneInvite = z.infer<typeof apiSchemas.shape.PhoneInvite>;
export type TraceFile = z.infer<typeof apiSchemas.shape.TraceFile>;

/** One JPEG frame and when it was captured. */
export interface CameraJpeg {
  jpeg: Uint8Array;
  /** Receipt time for simulator cameras; null when the service sent none. */
  capturedAtUnixNanoseconds: bigint | null;
}

const mediaStatusSchema = z.object({
  cameras: z.record(z.string(), z.object({ detail: z.string() })),
});

export function createStationHttpClient(
  options: { stationUrl?: string; mediaUrl?: string } = {},
) {
  const api = createStationApi(
    options.stationUrl ?? DEFAULT_STATION_URL,
    options.mediaUrl ?? DEFAULT_MEDIA_URL,
  );
  const stationUrl = options.stationUrl ?? DEFAULT_STATION_URL;
  /** The latest failure the station saw in the media programs' output for a camera. */
  async function cameraProblem(name: string): Promise<string | null> {
    try {
      const status = mediaStatusSchema.parse(
        await stationRequest(new URL("/viewer/api/media-status", stationUrl)),
      );
      return status.cameras[name]?.detail ?? null;
    } catch {
      return null;
    }
  }
  return {
    /** Names of the cameras streaming now. */
    async cameraNames(): Promise<string[]> {
      return (await api.listCameras())
        .filter(({ available }) => available)
        .map(({ name }) => name);
    },
    /** The newest JPEG frame from a camera. */
    async cameraJpeg(name: string): Promise<CameraJpeg> {
      const response = await api.getCamera(name).catch(async (error) => {
        if (error instanceof StationHttpError && error.status === 404)
          throw new CameraUnavailableError(
            name,
            await api.listCameras().catch(() => []),
            await cameraProblem(name),
          );
        throw error;
      });
      const capturedAt = response.headers.get(CAPTURED_AT_HEADER);
      return {
        jpeg: new Uint8Array(await response.arrayBuffer()),
        capturedAtUnixNanoseconds:
          capturedAt !== null && /^[0-9]+$/.test(capturedAt)
            ? BigInt(capturedAt)
            : null,
      };
    },
    /** An invite to stream a phone as camera `phone_<label>`. */
    phoneInvite: (label: string): Promise<PhoneInvite> =>
      api.phoneInvite({ label }),
    /** The executor's trace files, newest first. */
    listTraces: (): Promise<TraceFile[]> => api.listTraces(),
    /** A trace file's bytes exactly as served: gzip-compressed. */
    async downloadTrace(name: string): Promise<Uint8Array> {
      return new Uint8Array(await (await api.getTraceFile(name)).arrayBuffer());
    },
  };
}

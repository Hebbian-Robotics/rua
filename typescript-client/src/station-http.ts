import type { z } from "zod";
import { apiSchemas } from "./generated/api-schemas.js";
import { createStationApi } from "./generated/http-routes.js";
import { CameraUnavailableError, StationHttpError } from "./station-errors.js";
import { stationRequest } from "./station-request.js";

export const DEFAULT_STATION_URL = "http://127.0.0.1:8792";
/** Default direct media address for clients opting out of forwarding. */
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

function snapshotProblem(
  camera: z.infer<typeof apiSchemas.shape.CameraStatus>,
): string | null {
  switch (camera.state) {
    case "resolving":
      return "Looking up the camera";
    case "starting":
      return "Waiting for the first local frame";
    case "disconnected":
      return "USB camera disconnected (no matching device)";
    case "ambiguous":
      return "USB camera is ambiguous (multiple matching devices). Use rua camera-devices and pass --camera NAME=DEVICE";
    case "stalled":
      return "Local camera capture stalled. Retrying";
    case "failed":
      return camera.detail;
    case "capturing":
      switch (camera.publication.state) {
        case "connecting":
          return "Local frames available. Connecting to LiveKit";
        case "reconnecting":
          return camera.publication.detail === null
            ? "Local frames available. LiveKit reconnecting"
            : `Local frames available. LiveKit reconnecting: ${camera.publication.detail}`;
        case "disabled":
        case "publishing":
          return null;
      }
  }
}

export function createStationHttpClient(
  options: { stationUrl?: string; mediaUrl?: string } = {},
) {
  const stationUrl = options.stationUrl ?? DEFAULT_STATION_URL;
  const api = createStationApi(stationUrl, options.mediaUrl ?? stationUrl);
  async function cameraProblem(name: string): Promise<string | null> {
    try {
      const status = apiSchemas.shape.MediaStatusReport.parse(
        await stationRequest(new URL("/media/status", stationUrl)),
      );
      const camera = status.cameras[name];
      return camera ? snapshotProblem(camera) : null;
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
      const capturedAt =
        response.headers[CAPTURED_AT_HEADER.toLowerCase()] ?? null;
      return {
        jpeg: response.bytes,
        capturedAtUnixNanoseconds:
          capturedAt !== null && /^[0-9]+$/.test(capturedAt)
            ? BigInt(capturedAt)
            : null,
      };
    },
    /** An invite to stream a phone as camera `phone_<label>`. */
    phoneInvite: (label: string): Promise<PhoneInvite> =>
      api.phoneInvite({ label }),
    /** The station's trace files, newest first. */
    listTraces: (): Promise<TraceFile[]> => api.listTraces(),
    /** A trace file's bytes exactly as served: gzip-compressed. */
    async downloadTrace(name: string): Promise<Uint8Array> {
      return (await api.getTraceFile(name)).bytes;
    },
  };
}

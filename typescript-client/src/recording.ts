import { createStationApi } from "./generated/http-routes.js";

export function createRecordingClient(stationUrl: string) {
  const api = createStationApi(stationUrl, stationUrl);
  return {
    start: api.startRecording,
    status: api.recordingStatus,
    stop: api.stopRecording,
    ensure: api.ensureRecording,
  };
}

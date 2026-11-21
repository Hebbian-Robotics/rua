// The viewer's JSON calls. The station serves this page and answers every
// `/viewer/api` route itself (LiveKit access, phone invites, runs, recordings,
// the control mode), so a relative path reaches it directly, and through a
// reverse proxy that exposes /viewer/. These routes can start runs and issue
// camera tokens. The lease, state and live run use the station's HTTP API and
// event stream (station-connection.ts); protect them when allowing remote access.

import { stationRequest } from "@hebbian/robot-use-client";

export async function stationJson(
  path: string,
  init?: RequestInit,
): Promise<unknown> {
  return stationRequest(path, { cache: "no-store", ...init });
}

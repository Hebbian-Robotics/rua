import { z } from "zod";

import { StationHttpError } from "./station-errors.js";

const errorBodySchema = z.object({ detail: z.string() });

/** How long an HTTP request to the station may take. */
const HTTP_TIMEOUT_MILLISECONDS = 30_000;

function parseJsonOrNull(text: string): unknown {
  try {
    return JSON.parse(text);
  } catch {
    return null;
  }
}

/** Fetch from the station, throwing `StationHttpError` for an error status. */
export async function stationFetch(
  url: string | URL,
  init?: RequestInit,
): Promise<Response> {
  const response = await fetch(url, {
    signal: AbortSignal.timeout(HTTP_TIMEOUT_MILLISECONDS),
    ...init,
  });
  if (!response.ok) {
    const body = await response.text().catch(() => "");
    const detail = errorBodySchema.safeParse(parseJsonOrNull(body));
    throw new StationHttpError(
      response.status,
      body,
      detail.success ? detail.data.detail : `HTTP ${response.status}`,
    );
  }
  return response;
}

/**
 * Fetch JSON from the station; a failed request throws a `StationHttpError`
 * whose message is the station's `detail`, or "HTTP <status>".
 */
export async function stationRequest(
  url: string | URL,
  init?: RequestInit,
): Promise<unknown> {
  return (await stationFetch(url, init)).json();
}

/** RequestInit for a JSON body, or none when there is no body. */
export function jsonRequestInit(method: string, body?: unknown): RequestInit {
  return body === undefined
    ? { method }
    : {
        method,
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
      };
}

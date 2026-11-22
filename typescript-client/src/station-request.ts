import { z } from "zod";

import { StationHttpError } from "./station-errors.js";

const errorBodySchema = z.object({ detail: z.string() });

/** How long an HTTP request to the station may take. */
const HTTP_TIMEOUT_MILLISECONDS = 30_000;
const MAX_PLAIN_TEXT_ERROR_LENGTH = 500;

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
  const deadline = AbortSignal.timeout(HTTP_TIMEOUT_MILLISECONDS);
  const response = await fetch(url, {
    ...init,
    signal: init?.signal ? AbortSignal.any([deadline, init.signal]) : deadline,
  });
  if (!response.ok) {
    const body = await response.text().catch(() => "");
    const detail = errorBodySchema.safeParse(parseJsonOrNull(body));
    const contentType = response.headers.get("content-type")?.split(";")[0];
    const plainText =
      contentType === "text/plain"
        ? body.trim().slice(0, MAX_PLAIN_TEXT_ERROR_LENGTH)
        : "";
    throw new StationHttpError(
      response.status,
      body,
      detail.success
        ? detail.data.detail
        : plainText || `HTTP ${response.status}`,
    );
  }
  return response;
}

/**
 * Fetch JSON from the station; a failed request throws a `StationHttpError`
 * with the station's JSON detail or plain-text explanation when supplied.
 */
export async function stationRequest(
  url: string | URL,
  init?: RequestInit,
): Promise<unknown> {
  return (await stationFetch(url, init)).json();
}

/** RequestInit for a JSON body, or none when there is no body. */
export function jsonRequestInit(
  method: string,
  body?: unknown,
  signal?: AbortSignal,
): RequestInit {
  return body === undefined
    ? { method, signal }
    : {
        method,
        signal,
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
      };
}

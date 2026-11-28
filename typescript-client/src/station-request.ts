import { Effect } from "effect";
import { HttpClient } from "effect/http";
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

function stationHttpError(
  status: number,
  body: string,
  contentType: string | undefined,
): StationHttpError {
  const detail = errorBodySchema.safeParse(parseJsonOrNull(body));
  const plainText =
    contentType?.split(";")[0] === "text/plain"
      ? body.trim().slice(0, MAX_PLAIN_TEXT_ERROR_LENGTH)
      : "";
  return new StationHttpError(
    status,
    body,
    detail.success ? detail.data.detail : plainText || `HTTP ${status}`,
  );
}

/** Fetch and decode a JSON response within the current Effect HTTP scope. */
export function stationJsonEffect(
  url: URL,
): Effect.Effect<unknown, unknown, HttpClient.HttpClient> {
  return Effect.gen(function* () {
    const client = HttpClient.withScope(yield* HttpClient.HttpClient);
    const response = yield* client.get(url);
    if (response.status < 200 || response.status >= 300) {
      const body = yield* response.text.pipe(Effect.orElseSucceed(() => ""));
      return yield* Effect.fail(
        stationHttpError(
          response.status,
          body,
          response.headers["content-type"],
        ),
      );
    }
    return yield* response.json;
  }).pipe(Effect.scoped);
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
    throw stationHttpError(
      response.status,
      body,
      response.headers.get("content-type") ?? undefined,
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
